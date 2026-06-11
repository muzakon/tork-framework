//! Request body-size limit middleware.

use http::header::CONTENT_LENGTH;

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
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());

        if matches!(declared, Some(length) if length > self.limit) {
            return Box::pin(async { Err(Error::payload_too_large("request body too large")) });
        }

        next.run(request)
    }

    fn name(&self) -> &'static str {
        "BodyLimit"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_scale_bytes_kb_and_mb() {
        assert_eq!(BodyLimit::bytes(7).limit, 7);
        assert_eq!(BodyLimit::kb(2).limit, 2 * BYTES_PER_KB);
        assert_eq!(BodyLimit::mb(3).limit, 3 * BYTES_PER_MB);
    }
}
