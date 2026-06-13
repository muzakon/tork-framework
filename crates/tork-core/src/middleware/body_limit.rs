//! Request body-size limit middleware.

use bytes::Bytes;
use http::header::CONTENT_LENGTH;
use http_body_util::BodyExt;
use http_body_util::Full;

use crate::body::{box_body, ReqBody};
use crate::error::{Error, Result};
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Bytes in a kibibyte.
const BYTES_PER_KB: usize = 1024;
/// Bytes in a mebibyte.
const BYTES_PER_MB: usize = 1024 * 1024;

/// Rejects requests whose declared body size exceeds a limit.
///
/// The `Content-Length` header is checked before the handler runs; an oversized
/// request is rejected with `413 Payload Too Large`. Requests without a
/// `Content-Length` (for example chunked uploads) are still bounded by the body
/// extractor's own cap.
pub struct BodyLimit {
    limit: usize,
}

impl BodyLimit {
    /// Creates a limit of `limit` bytes.
    pub fn bytes(limit: usize) -> Self {
        Self { limit }
    }

    /// Creates a limit of `limit` kibibytes.
    pub fn kb(limit: usize) -> Self {
        Self {
            limit: limit * BYTES_PER_KB,
        }
    }

    /// Creates a limit of `limit` mebibytes.
    pub fn mb(limit: usize) -> Self {
        Self {
            limit: limit * BYTES_PER_MB,
        }
    }
}

impl Middleware for BodyLimit {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let limit = self.limit;
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());

        if matches!(declared, Some(length) if length > limit) {
            return Box::pin(async { Err(Error::payload_too_large("request body too large")) });
        }

        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let bytes = read_body_with_limit(body, limit).await?;
            let request = Request::from_parts(parts, box_body(Full::new(bytes)));
            next.run(request).await
        })
    }

    fn name(&self) -> &'static str {
        "BodyLimit"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

async fn read_body_with_limit(mut body: ReqBody, limit: usize) -> Result<Bytes> {
    let mut seen = 0usize;
    let mut buffer = Vec::new();

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| match error.downcast::<Error>() {
            Ok(error) => *error,
            Err(_) => Error::bad_request("request body could not be read"),
        })?;

        if let Some(data) = frame.data_ref() {
            seen += data.len();
            if seen > limit {
                return Err(Error::payload_too_large("request body too large"));
            }
            buffer.extend_from_slice(data);
        }
    }

    Ok(Bytes::from(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use futures_util::stream;
    use http_body::Frame;

    #[test]
    fn constructors_scale_bytes_kb_and_mb() {
        assert_eq!(BodyLimit::bytes(7).limit, 7);
        assert_eq!(BodyLimit::kb(2).limit, 2 * BYTES_PER_KB);
        assert_eq!(BodyLimit::mb(3).limit, 3 * BYTES_PER_MB);
    }

    #[tokio::test]
    async fn read_body_with_limit_errors_after_crossing_limit() {
        let chunks = vec![
            Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(b"he"))),
            Ok(Frame::data(Bytes::from_static(b"llo"))),
        ];
        let body = box_body(http_body_util::StreamBody::new(stream::iter(chunks)));
        let error = read_body_with_limit(body, 4).await.unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::PayloadTooLarge);
    }
}
