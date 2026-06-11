//! Cross-Origin Resource Sharing (CORS) middleware.

use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE,
    ACCESS_CONTROL_REQUEST_METHOD, ORIGIN, VARY,
};
use http::{HeaderValue, Method, StatusCode};

use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::{Response, empty};
use crate::router::BoxFuture;

/// The wildcard origin token.
const WILDCARD: &str = "*";

/// Adds CORS headers and answers preflight (`OPTIONS`) requests.
///
/// Configure the allowed origins, methods, headers, exposed headers, and
/// credentials via the builder. A preflight request is answered directly with
/// `204 No Content` and the negotiated `Access-Control-*` headers; the route
/// handler is not invoked. Actual requests are annotated with the allowed origin
/// and exposed headers.
pub struct Cors {
    origins: Vec<String>,
    methods: Option<HeaderValue>,
    headers: Option<HeaderValue>,
    expose: Option<HeaderValue>,
    credentials: bool,
    max_age: Option<HeaderValue>,
}

impl Cors {
    /// Creates a CORS middleware with no origins allowed yet.
    pub fn new() -> Self {
        Self {
            origins: Vec::new(),
            methods: None,
            headers: None,
            expose: None,
            credentials: false,
            max_age: None,
        }
    }

    /// Allows an origin (call repeatedly), or `"*"` to allow any origin.
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.origins.push(origin.into());
        self
    }

    /// Sets the methods allowed for cross-origin requests.
    pub fn allow_methods<I, S>(mut self, methods: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.methods = join(methods);
        self
    }

    /// Sets the request headers allowed for cross-origin requests.
    pub fn allow_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.headers = join(headers);
        self
    }

    /// Sets the response headers exposed to the client.
    pub fn expose_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.expose = join(headers);
        self
    }

    /// Allows credentials (cookies, authorization headers) on cross-origin requests.
    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.credentials = allow;
        self
    }

    /// Sets how long (in seconds) a preflight result may be cached.
    pub fn max_age(mut self, seconds: u64) -> Self {
        self.max_age = HeaderValue::from_str(&seconds.to_string()).ok();
        self
    }

    /// Resolves the `Access-Control-Allow-Origin` value for a request, if any.
    fn allow_origin_value(&self, request: &Request) -> Option<HeaderValue> {
        let origin = request.headers().get(ORIGIN)?.to_str().ok()?;
        let any = self.origins.iter().any(|o| o == WILDCARD);
        let allowed = any || self.origins.iter().any(|o| o == origin);
        if !allowed {
            return None;
        }
        // `*` cannot be combined with credentials; echo the origin in that case.
        if any && !self.credentials {
            Some(HeaderValue::from_static(WILDCARD))
        } else {
            HeaderValue::from_str(origin).ok()
        }
    }
}

impl Default for Cors {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for Cors {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let allow_origin = self.allow_origin_value(&request);
        let is_preflight = request.method() == Method::OPTIONS
            && request.headers().contains_key(ACCESS_CONTROL_REQUEST_METHOD);

        if is_preflight {
            let mut response = empty(StatusCode::NO_CONTENT);
            let headers = response.headers_mut();
            if let Some(origin) = allow_origin {
                headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                headers.insert(VARY, HeaderValue::from_static("Origin"));
            }
            if let Some(methods) = &self.methods {
                headers.insert(ACCESS_CONTROL_ALLOW_METHODS, methods.clone());
            }
            if let Some(allowed) = &self.headers {
                headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, allowed.clone());
            }
            if self.credentials {
                headers.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
            }
            if let Some(max_age) = &self.max_age {
                headers.insert(ACCESS_CONTROL_MAX_AGE, max_age.clone());
            }
            return Box::pin(async move { Ok(response) });
        }

        let expose = self.expose.clone();
        let credentials = self.credentials;
        Box::pin(async move {
            let mut response = next.run(request).await?;
            if let Some(origin) = allow_origin {
                let headers = response.headers_mut();
                headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                headers.insert(VARY, HeaderValue::from_static("Origin"));
                if let Some(expose) = expose {
                    headers.insert(ACCESS_CONTROL_EXPOSE_HEADERS, expose);
                }
                if credentials {
                    headers.insert(
                        ACCESS_CONTROL_ALLOW_CREDENTIALS,
                        HeaderValue::from_static("true"),
                    );
                }
            }
            Ok(response)
        })
    }

    fn name(&self) -> &'static str {
        "Cors"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

/// Joins items into a single comma-separated header value.
fn join<I, S>(items: I) -> Option<HeaderValue>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let joined = items
        .into_iter()
        .map(|item| item.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        None
    } else {
        HeaderValue::from_str(&joined).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::Full;
    use crate::body::box_body;

    fn request(origin: Option<&str>) -> Request {
        let mut builder = http::Request::builder().method(Method::GET).uri("/");
        if let Some(origin) = origin {
            builder = builder.header(ORIGIN, origin);
        }
        builder.body(box_body(Full::new(Bytes::new()))).unwrap()
    }

    #[test]
    fn join_builds_header_values_or_none() {
        assert_eq!(join(["GET", "POST"]).unwrap(), "GET, POST");
        assert!(join::<[&str; 0], _>([]).is_none());
    }

    #[test]
    fn wildcard_without_credentials_returns_star() {
        let cors = Cors::new().allow_origin("*");
        let value = cors.allow_origin_value(&request(Some("https://app.example.com")));

        assert_eq!(value.unwrap(), "*");
    }

    #[test]
    fn wildcard_with_credentials_echoes_origin() {
        let cors = Cors::new().allow_origin("*").allow_credentials(true);
        let value = cors.allow_origin_value(&request(Some("https://app.example.com")));

        assert_eq!(value.unwrap(), "https://app.example.com");
    }

    #[test]
    fn exact_allow_list_rejects_unknown_origin() {
        let cors = Cors::new().allow_origin("https://good.example.com");

        assert!(cors.allow_origin_value(&request(Some("https://evil.example.com"))).is_none());
        assert!(cors.allow_origin_value(&request(None)).is_none());
    }
}
