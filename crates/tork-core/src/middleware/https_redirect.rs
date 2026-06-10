//! HTTPS-redirect middleware.

use bytes::Bytes;
use http::header::{HOST, LOCATION};
use http::{HeaderValue, StatusCode};

use crate::constants::TEXT_PLAIN_UTF8;
use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::{Response, bytes_response};
use crate::router::BoxFuture;

/// Header set by a terminating proxy to convey the original scheme.
const FORWARDED_PROTO: &str = "x-forwarded-proto";

/// Redirects plain-HTTP requests to HTTPS with a `308 Permanent Redirect`.
///
/// The scheme is taken from the `X-Forwarded-Proto` header (set by a terminating
/// proxy) when present, otherwise from the request URI.
pub struct HttpsRedirect;

impl HttpsRedirect {
    /// Creates the middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpsRedirect {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for HttpsRedirect {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        if is_https(&request) {
            return next.run(request);
        }

        let host = request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let path = request
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        let location = format!("https://{host}{path}");

        let mut response = bytes_response(StatusCode::PERMANENT_REDIRECT, TEXT_PLAIN_UTF8, Bytes::new());
        if let Ok(value) = HeaderValue::from_str(&location) {
            response.headers_mut().insert(LOCATION, value);
        }
        Box::pin(async move { Ok(response) })
    }

    fn name(&self) -> &'static str {
        "HttpsRedirect"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

/// Returns `true` if the request arrived over HTTPS.
fn is_https(request: &Request) -> bool {
    if let Some(proto) = request
        .headers()
        .get(FORWARDED_PROTO)
        .and_then(|value| value.to_str().ok())
    {
        return proto.eq_ignore_ascii_case("https");
    }
    request.uri().scheme_str() == Some("https")
}
