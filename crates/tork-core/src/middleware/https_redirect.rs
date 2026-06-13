//! HTTPS-redirect middleware.

use bytes::Bytes;
use http::header::{HOST, LOCATION};
use http::{HeaderValue, StatusCode};

use crate::constants::TEXT_PLAIN_UTF8;
use crate::error::Result;
use crate::extract::scheme_from_extensions;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::{bytes_response, Response};
use crate::router::BoxFuture;

/// Redirects plain-HTTP requests to HTTPS with a `308 Permanent Redirect`.
///
/// The scheme is taken from trusted proxy normalization when present, otherwise
/// from the request URI.
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

        let mut response = bytes_response(
            StatusCode::PERMANENT_REDIRECT,
            TEXT_PLAIN_UTF8,
            Bytes::new(),
        );
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
    if let Some(scheme) = scheme_from_extensions(request.extensions()) {
        return scheme == crate::extract::RequestScheme::Https;
    }
    request.uri().scheme_str() == Some("https")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::extract::RequestScheme;
    use http_body_util::Full;

    fn request(uri: &str, scheme: Option<RequestScheme>) -> Request {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(uri)
            .body(box_body(Full::new(Bytes::new())))
            .unwrap();
        if let Some(scheme) = scheme {
            request.extensions_mut().insert(scheme);
        }
        request
    }

    #[test]
    fn trusted_scheme_extension_takes_priority() {
        assert!(is_https(&request("/", Some(RequestScheme::Https))));
        assert!(!is_https(&request("/", Some(RequestScheme::Http))));
    }

    #[test]
    fn uri_scheme_is_used_without_proxy_header() {
        assert!(is_https(&request("https://example.com/path", None)));
        assert!(!is_https(&request("http://example.com/path", None)));
        assert!(!is_https(&request("/path", None)));
    }
}
