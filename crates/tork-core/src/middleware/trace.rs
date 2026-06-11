//! Request-tracing middleware.

use std::time::Instant;

use crate::error::Result;
use crate::logging::Logger;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Logs each request's method, path, resulting status, and elapsed time.
///
/// This logs before routing (so it covers short-circuited and unmatched requests);
/// the framework's built-in HTTP log already covers matched routes, so this
/// middleware is only needed when pre-routing coverage is wanted.
pub struct Trace;

impl Trace {
    /// Creates the tracing middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for Trace {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for Trace {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        let start = Instant::now();

        Box::pin(async move {
            let result = next.run(request).await;
            let elapsed = start.elapsed();
            let status = match &result {
                Ok(response) => response.status(),
                Err(error) => error.kind().status(),
            };
            Logger::framework("HTTP")
                .info(format!("{method} {path} {}", status.as_u16()))
                .field("method", method.as_str())
                .field("path", &path)
                .field("status", status.as_u16())
                .field("duration_ms", elapsed.as_millis() as u64)
                .emit();
            result
        })
    }

    fn name(&self) -> &'static str {
        "Trace"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_metadata_is_stable() {
        let middleware = Trace::new();
        assert_eq!(middleware.name(), "Trace");
        assert_eq!(middleware.duplicate_policy(), DuplicatePolicy::Reject);
    }
}
