//! Request-lifecycle observability hooks and their event contexts.
//!
//! Hooks are observe-only callbacks the application registers to watch the
//! request lifecycle for logging, metrics, or tracing. They receive an event
//! context carrying request metadata (and, where relevant, the response status,
//! elapsed time, error, or panic message) and cannot alter the response.
//!
//! The contexts own a snapshot of their data so that a hook may move the context
//! into a `'static` future. None of them expose the request or response body.

use std::time::Duration;

use http::{Method, StatusCode};

use crate::error::ErrorDetail;

/// Request metadata shared by every hook event.
///
/// Built once per request and cloned into each event that fires for it.
#[derive(Clone, Debug)]
pub(crate) struct RequestInfo {
    method: Method,
    path: String,
    route: Option<String>,
    request_id: Option<String>,
}

impl RequestInfo {
    /// Creates request metadata for the current request.
    pub(crate) fn new(
        method: Method,
        path: String,
        route: Option<String>,
        request_id: Option<String>,
    ) -> Self {
        Self {
            method,
            path,
            route,
            request_id,
        }
    }

}

/// Generates the request-metadata accessors shared by every event context.
macro_rules! shared_accessors {
    ($t:ty) => {
        impl $t {
            /// The HTTP method of the request.
            pub fn method(&self) -> &Method {
                &self.info.method
            }

            /// The request path (the concrete URI path, not the route pattern).
            pub fn path(&self) -> &str {
                &self.info.path
            }

            /// The matched route pattern (for example `/users/{id}`), if routing
            /// resolved one.
            pub fn route(&self) -> Option<&str> {
                self.info.route.as_deref()
            }

            /// The request identifier (the `x-request-id` value), if present.
            pub fn request_id(&self) -> Option<&str> {
                self.info.request_id.as_deref()
            }
        }
    };
}

/// Context for [`on_request`](crate::App::on_request): a request has arrived.
pub struct RequestEvent {
    info: RequestInfo,
}

impl RequestEvent {
    pub(crate) fn new(info: RequestInfo) -> Self {
        Self { info }
    }
}

shared_accessors!(RequestEvent);

/// Context for [`on_response`](crate::App::on_response): a response is ready.
pub struct ResponseEvent {
    info: RequestInfo,
    status: StatusCode,
    elapsed: Duration,
}

impl ResponseEvent {
    pub(crate) fn new(info: RequestInfo, status: StatusCode, elapsed: Duration) -> Self {
        Self {
            info,
            status,
            elapsed,
        }
    }

    /// The status code of the response being returned.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// How long the request took, measured from the start of handling.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

shared_accessors!(ResponseEvent);

/// Context for [`on_error`](crate::App::on_error): a non-validation error was
/// produced.
pub struct ErrorEvent {
    info: RequestInfo,
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ErrorEvent {
    pub(crate) fn new(
        info: RequestInfo,
        status: StatusCode,
        code: &'static str,
        message: String,
    ) -> Self {
        Self {
            info,
            status,
            code,
            message,
        }
    }

    /// The HTTP status the error renders to.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The machine-readable error code (for example `NOT_FOUND`).
    pub fn code(&self) -> &str {
        self.code
    }

    /// The server-side error message (not necessarily what the client receives).
    pub fn message(&self) -> &str {
        &self.message
    }
}

shared_accessors!(ErrorEvent);

/// Context for [`on_validation_error`](crate::App::on_validation_error): a
/// request body failed validation (`422`).
pub struct ValidationErrorEvent {
    info: RequestInfo,
    details: Vec<ErrorDetail>,
}

impl ValidationErrorEvent {
    pub(crate) fn new(info: RequestInfo, details: Vec<ErrorDetail>) -> Self {
        Self { info, details }
    }

    /// The field-level validation failures.
    pub fn details(&self) -> &[ErrorDetail] {
        &self.details
    }
}

shared_accessors!(ValidationErrorEvent);

/// Context for [`on_panic`](crate::App::on_panic): a handler panicked and the
/// panic was caught by the panic boundary.
pub struct PanicEvent {
    info: RequestInfo,
    message: String,
}

impl PanicEvent {
    pub(crate) fn new(info: RequestInfo, message: String) -> Self {
        Self { info, message }
    }

    /// The panic payload rendered as text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

shared_accessors!(PanicEvent);

/// Context passed to an [`exception_handler`](crate::App::exception_handler)
/// alongside the recovered typed error.
pub struct ErrorContext {
    info: RequestInfo,
}

impl ErrorContext {
    pub(crate) fn new(info: RequestInfo) -> Self {
        Self { info }
    }
}

shared_accessors!(ErrorContext);

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> RequestInfo {
        RequestInfo::new(
            Method::GET,
            "/users/7".to_owned(),
            Some("/users/{id}".to_owned()),
            Some("req-1".to_owned()),
        )
    }

    #[test]
    fn shared_accessors_expose_request_metadata() {
        let event = RequestEvent::new(info());
        assert_eq!(event.method(), Method::GET);
        assert_eq!(event.path(), "/users/7");
        assert_eq!(event.route(), Some("/users/{id}"));
        assert_eq!(event.request_id(), Some("req-1"));
    }

    #[test]
    fn response_event_carries_status_and_elapsed() {
        let event = ResponseEvent::new(info(), StatusCode::OK, Duration::from_millis(5));
        assert_eq!(event.status(), StatusCode::OK);
        assert_eq!(event.elapsed(), Duration::from_millis(5));
    }

    #[test]
    fn error_event_carries_status_code_and_message() {
        let event = ErrorEvent::new(info(), StatusCode::NOT_FOUND, "NOT_FOUND", "missing".to_owned());
        assert_eq!(event.status(), StatusCode::NOT_FOUND);
        assert_eq!(event.code(), "NOT_FOUND");
        assert_eq!(event.message(), "missing");
    }

    #[test]
    fn validation_event_carries_details() {
        let event = ValidationErrorEvent::new(
            info(),
            vec![ErrorDetail::new("name", "TOO_SHORT", "too short")],
        );
        assert_eq!(event.details().len(), 1);
        assert_eq!(event.details()[0].field, "name");
    }

    #[test]
    fn panic_event_carries_message() {
        let event = PanicEvent::new(info(), "boom".to_owned());
        assert_eq!(event.message(), "boom");
    }

    #[test]
    fn error_context_exposes_route() {
        let ctx = ErrorContext::new(info());
        assert_eq!(ctx.route(), Some("/users/{id}"));
    }
}
