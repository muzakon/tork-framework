//! Proxy-header normalization middleware.

use http::header::HOST;

use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Header conveying the original host through a terminating proxy.
const FORWARDED_HOST: &str = "x-forwarded-host";

/// Normalizes proxy-forwarded headers onto the request.
///
/// When the request comes through a terminating proxy, the original host arrives
/// in `X-Forwarded-Host`; this middleware rewrites the `Host` header from it so
/// downstream host-based middleware (such as [`TrustedHost`](super::TrustedHost))
/// sees the client-facing host. Register it before those middlewares. The
/// forwarded scheme is honored directly by
/// [`HttpsRedirect`](super::HttpsRedirect).
pub struct ProxyHeaders;

impl ProxyHeaders {
    /// Creates the middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProxyHeaders {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for ProxyHeaders {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        if let Some(forwarded_host) = request.headers().get(FORWARDED_HOST).cloned() {
            request.headers_mut().insert(HOST, forwarded_host);
        }
        next.run(request)
    }

    fn name(&self) -> &'static str {
        "ProxyHeaders"
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
        let middleware = ProxyHeaders::new();
        assert_eq!(middleware.name(), "ProxyHeaders");
        assert_eq!(middleware.duplicate_policy(), DuplicatePolicy::Reject);
    }
}
