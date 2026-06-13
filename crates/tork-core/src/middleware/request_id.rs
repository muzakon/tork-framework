//! Request-identifier middleware.

use http::{HeaderName, HeaderValue};

use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Default header carrying the request identifier.
const DEFAULT_HEADER: &str = "x-request-id";
/// Prefix applied to generated request identifiers.
const REQUEST_ID_PREFIX: &str = "req-";

/// Assigns a request identifier and echoes it on the response.
///
/// If the incoming request already carries the id header, its value is reused;
/// otherwise a `req-<uuid>` value is generated. The id is written back onto the
/// request (so downstream layers and handlers can read it) and onto the
/// response.
pub struct RequestId {
    header: HeaderName,
}

impl RequestId {
    /// Creates the middleware using the default `x-request-id` header.
    pub fn new() -> Self {
        Self {
            header: HeaderName::from_static(DEFAULT_HEADER),
        }
    }

    /// Sets the header name that carries the request id.
    pub fn header_name(mut self, name: &'static str) -> Self {
        self.header = HeaderName::from_static(name);
        self
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for RequestId {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let header = self.header.clone();

        let id = request
            .headers()
            .get(&header)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{REQUEST_ID_PREFIX}{}", uuid::Uuid::new_v4()));

        if let Ok(value) = HeaderValue::from_str(&id) {
            request.headers_mut().insert(header.clone(), value);
        }

        Box::pin(async move {
            let mut response = next.run(request).await?;
            if let Ok(value) = HeaderValue::from_str(&id) {
                response.headers_mut().insert(header, value);
            }
            Ok(response)
        })
    }

    fn name(&self) -> &'static str {
        "RequestId"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_name_builder_replaces_default_header() {
        let request_id = RequestId::new().header_name("x-correlation-id");
        assert_eq!(
            request_id.header,
            HeaderName::from_static("x-correlation-id")
        );
    }
}
