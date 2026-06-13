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

    /// Builds a streaming body that fails once it has emitted more than
    /// `max_bytes`.
    ///
    /// Unbounded streaming responses (a file download backed by a generator, say)
    /// have no inherent size limit; a runaway or buggy producer can stream without
    /// end. Wrapping the body caps the total bytes it may emit, erroring the
    /// response stream past the limit so it cannot run forever. Server-Sent Events
    /// are intentionally open-ended and use [`stream`](RespBody::stream) instead.
    pub fn stream_capped<B>(body: B, max_bytes: u64) -> Self
    where
        B: Body<Data = Bytes, Error = BoxError> + Send + 'static,
    {
        Self {
            kind: BodyKind::Stream(Box::pin(CappedBody {
                inner: Box::pin(body),
                emitted: 0,
                limit: max_bytes,
            })),
        }
    }
}

/// A streaming body that errors once it has emitted more than its byte limit.
struct CappedBody {
    inner: BoxStreamBody,
    emitted: u64,
    limit: u64,
}

impl Body for CappedBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.emitted = this.emitted.saturating_add(data.len() as u64);
                    if this.emitted > this.limit {
                        return Poll::Ready(Some(Err(format!(
                            "response body exceeded the {}-byte limit",
                            this.limit
                        )
                        .into())));
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
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

    #[tokio::test]
    async fn capped_stream_errors_once_it_exceeds_the_limit() {
        // Three 4-byte frames (12 bytes total) under a 10-byte cap: the first two
        // pass, and the frame that pushes the total over the limit errors.
        let frames = futures_util::stream::iter(vec![
            Ok::<_, BoxError>(Frame::data(Bytes::from_static(b"aaaa"))),
            Ok(Frame::data(Bytes::from_static(b"bbbb"))),
            Ok(Frame::data(Bytes::from_static(b"cccc"))),
        ]);
        let mut body = RespBody::stream_capped(StreamBody::new(frames), 10);

        let mut delivered = 0usize;
        let mut errored = false;
        loop {
            let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
            match frame {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        delivered += data.len();
                    }
                }
                Some(Err(_)) => {
                    errored = true;
                    break;
                }
                None => break,
            }
        }

        assert!(errored, "the body should error once it exceeds the cap");
        assert_eq!(delivered, 8, "only the frames within the cap are delivered");
    }
}
