//! Named constants used across the runtime.
//!
//! Centralizing these values avoids magic literals scattered through the code
//! and keeps content types, limits, and user-facing messages consistent.

use std::time::Duration;

/// `Content-Type` value for JSON payloads.
pub const APPLICATION_JSON: &str = "application/json";

/// `Content-Type` value for UTF-8 plain text.
pub const TEXT_PLAIN_UTF8: &str = "text/plain; charset=utf-8";

/// `Content-Type` value for UTF-8 HTML documents.
pub const TEXT_HTML_UTF8: &str = "text/html; charset=utf-8";

/// `Content-Type` value for UTF-8 JavaScript sources.
pub const APPLICATION_JAVASCRIPT_UTF8: &str = "application/javascript; charset=utf-8";

/// `Content-Type` value for a Server-Sent Events stream.
pub const TEXT_EVENT_STREAM: &str = "text/event-stream";

/// Prefix of an `Authorization` header value that carries a bearer token.
pub const BEARER_PREFIX: &str = "Bearer ";

/// Generic message returned to clients for any server-side (5xx) error.
///
/// Server errors never expose internal detail in the response body; only this
/// fixed message is sent while the real cause is logged server-side.
pub const INTERNAL_ERROR_MESSAGE: &str = "Internal server error";

/// Maximum number of bytes the framework buffers from a single request body.
///
/// Requests whose body exceeds this limit are rejected, guarding against
/// memory-exhaustion attacks.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Maximum time to wait for in-flight connections to drain during shutdown.
pub const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_constants_match_runtime_contract() {
        assert_eq!(APPLICATION_JSON, "application/json");
        assert_eq!(TEXT_EVENT_STREAM, "text/event-stream");
        assert_eq!(BEARER_PREFIX, "Bearer ");
        assert_eq!(INTERNAL_ERROR_MESSAGE, "Internal server error");
        assert_eq!(MAX_BODY_BYTES, 2 * 1024 * 1024);
        assert_eq!(GRACEFUL_SHUTDOWN_TIMEOUT, Duration::from_secs(15));
    }
}
