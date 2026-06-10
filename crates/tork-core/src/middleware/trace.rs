//! Request-tracing middleware.

use std::time::Instant;

use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Logs each request's method, path, resulting status, and elapsed time.
///
/// This is the framework's minimal default tracing sink (standard error); a
/// pluggable logging hook is planned for a later phase.
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
            eprintln!("tork: {method} {path} -> {} ({elapsed:?})", status.as_u16());
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
