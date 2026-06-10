//! Body type aliases shared across the runtime.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::UnsyncBoxBody;

/// Boxed, thread-safe error type carried by an erased request body.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The inbound request body type.
///
/// The concrete body produced by Hyper is erased into this boxed body so the
/// runtime is agnostic to where a request body comes from. This also makes the
/// request context easy to construct in tests.
pub type ReqBody = UnsyncBoxBody<Bytes, BoxError>;

/// A boxed streaming response body.
type BoxStreamBody = Pin<Box<dyn Body<Data = Bytes, Error = BoxError> + Send>>;

/// The outbound response body.
///
/// A response body is either fully buffered (the common case: a JSON payload, an
/// error body, a static asset) or streaming (Server-Sent Events, and other
/// frame-at-a-time responses). Both share this type, so handler and middleware
/// signatures do not change between them.
pub struct RespBody {
    kind: BodyKind,
}

/// The two shapes a [`RespBody`] can take.
enum BodyKind {
    /// A single, contiguous, already-available buffer.
    Full(Full<Bytes>),
    /// A body that yields frames over time.
    Stream(BoxStreamBody),
}

impl RespBody {
    /// Builds a fully-buffered body from a contiguous buffer.
    pub fn new(body: Bytes) -> Self {
        Self {
            kind: BodyKind::Full(Full::new(body)),
        }
    }

    /// Builds a streaming body that yields frames over time.
    ///
    /// Used by streaming responses such as Server-Sent Events, and available for
    /// returning a custom frame-at-a-time body.
    pub fn stream<B>(body: B) -> Self
    where
        B: Body<Data = Bytes, Error = BoxError> + Send + 'static,
    {
        Self {
            kind: BodyKind::Stream(Box::pin(body)),
        }
    }
}

impl Body for RespBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // `RespBody` is `Unpin` (its variants are), so projecting through `get_mut`
        // is sound and keeps the delegation simple.
        match &mut self.get_mut().kind {
            // `Full`'s error type is `Infallible`, so it never yields an error.
            BodyKind::Full(full) => Pin::new(full)
                .poll_frame(cx)
                .map_err(|never| match never {}),
            BodyKind::Stream(stream) => stream.as_mut().poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            BodyKind::Full(full) => full.is_end_stream(),
            BodyKind::Stream(stream) => stream.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            BodyKind::Full(full) => full.size_hint(),
            BodyKind::Stream(stream) => stream.size_hint(),
        }
    }
}

/// Erases any compatible HTTP body into the runtime's [`ReqBody`] type.
pub fn box_body<B>(body: B) -> ReqBody
where
    B: hyper::body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    body.map_err(Into::into).boxed_unsync()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::StreamBody;

    async fn collect_chunks(body: RespBody) -> Vec<Bytes> {
        let collected = body.collect().await.expect("body collects");
        // Re-stream the aggregated bytes as a single chunk for a simple assertion.
        vec![collected.to_bytes()]
    }

    #[tokio::test]
    async fn full_body_yields_its_buffer() {
        let body = RespBody::new(Bytes::from_static(b"hello"));
        let chunks = collect_chunks(body).await;
        assert_eq!(chunks, vec![Bytes::from_static(b"hello")]);
    }

    #[tokio::test]
    async fn streaming_body_yields_each_frame() {
        // A stream of three data frames, erased into a streaming RespBody.
        let frames = futures_util::stream::iter(vec![
            Ok::<_, BoxError>(Frame::data(Bytes::from_static(b"a"))),
            Ok(Frame::data(Bytes::from_static(b"b"))),
            Ok(Frame::data(Bytes::from_static(b"c"))),
        ]);
        let body = RespBody::stream(StreamBody::new(frames));

        let mut out = Vec::new();
        let mut body = body;
        loop {
            let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
            match frame {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        out.push(data);
                    }
                }
                Some(Err(error)) => panic!("unexpected body error: {error}"),
                None => break,
            }
        }

        assert_eq!(
            out,
            vec![
                Bytes::from_static(b"a"),
                Bytes::from_static(b"b"),
                Bytes::from_static(b"c"),
            ]
        );
    }
}
