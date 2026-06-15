//! Error type and HTTP error responses.

use std::any::TypeId;

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use crate::constants::{APPLICATION_JSON, INTERNAL_ERROR_MESSAGE};
use crate::response::{with_body, IntoResponse, Response};

/// A specialized [`Result`](core::result::Result) whose error type defaults to
/// [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Machine-readable code used for validation failures.
const VALIDATION_ERROR_CODE: &str = "VALIDATION_ERROR";
/// Default top-level message for a validation failure.
const VALIDATION_ERROR_MESSAGE: &str = "The submitted data failed validation.";
/// Issue code used for a field error that could not be classified.
const GENERIC_ISSUE: &str = "INVALID";
/// Prefix applied to generated trace identifiers.
const TRACE_ID_PREFIX: &str = "req-";

/// The category of an [`Error`], which determines the HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `400 Bad Request`.
    BadRequest,
    /// `401 Unauthorized`.
    Unauthorized,
    /// `403 Forbidden`.
    Forbidden,
    /// `404 Not Found`.
    NotFound,
    /// `405 Method Not Allowed`.
    MethodNotAllowed,
    /// `409 Conflict`.
    Conflict,
    /// `413 Payload Too Large`.
    PayloadTooLarge,
    /// `422 Unprocessable Entity`.
    Unprocessable,
    /// `429 Too Many Requests`.
    TooManyRequests,
    /// `500 Internal Server Error`.
    Internal,
    /// `503 Service Unavailable`.
    ServiceUnavailable,
    /// `504 Gateway Timeout`.
    GatewayTimeout,
}

impl ErrorKind {
    /// Returns the HTTP status code for this error category.
    pub fn status(self) -> StatusCode {
        match self {
            ErrorKind::BadRequest => StatusCode::BAD_REQUEST,
            ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorKind::Forbidden => StatusCode::FORBIDDEN,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ErrorKind::Conflict => StatusCode::CONFLICT,
            ErrorKind::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorKind::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    /// Returns the default stable, machine-readable code for this category.
    ///
    /// Codes are upper snake case (for example `NOT_FOUND`). An [`Error`] may
    /// override this with a more specific code via [`Error::with_code`].
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "BAD_REQUEST",
            ErrorKind::Unauthorized => "UNAUTHORIZED",
            ErrorKind::Forbidden => "FORBIDDEN",
            ErrorKind::NotFound => "NOT_FOUND",
            ErrorKind::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            ErrorKind::Conflict => "CONFLICT",
            ErrorKind::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            ErrorKind::Unprocessable => "UNPROCESSABLE_ENTITY",
            ErrorKind::TooManyRequests => "TOO_MANY_REQUESTS",
            ErrorKind::Internal => "INTERNAL_SERVER_ERROR",
            ErrorKind::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            ErrorKind::GatewayTimeout => "GATEWAY_TIMEOUT",
        }
    }

    /// Returns the kind that matches an HTTP status, if one does.
    ///
    /// Used by [`Error::http`] to pick a default machine code for an explicit
    /// status. Statuses without a named kind return `None`.
    fn from_status(status: StatusCode) -> Option<ErrorKind> {
        match status {
            StatusCode::BAD_REQUEST => Some(ErrorKind::BadRequest),
            StatusCode::UNAUTHORIZED => Some(ErrorKind::Unauthorized),
            StatusCode::FORBIDDEN => Some(ErrorKind::Forbidden),
            StatusCode::NOT_FOUND => Some(ErrorKind::NotFound),
            StatusCode::METHOD_NOT_ALLOWED => Some(ErrorKind::MethodNotAllowed),
            StatusCode::CONFLICT => Some(ErrorKind::Conflict),
            StatusCode::PAYLOAD_TOO_LARGE => Some(ErrorKind::PayloadTooLarge),
            StatusCode::UNPROCESSABLE_ENTITY => Some(ErrorKind::Unprocessable),
            StatusCode::TOO_MANY_REQUESTS => Some(ErrorKind::TooManyRequests),
            StatusCode::INTERNAL_SERVER_ERROR => Some(ErrorKind::Internal),
            StatusCode::SERVICE_UNAVAILABLE => Some(ErrorKind::ServiceUnavailable),
            StatusCode::GATEWAY_TIMEOUT => Some(ErrorKind::GatewayTimeout),
            _ => None,
        }
    }
}

/// A framework error that can be turned into an HTTP error response.
///
/// The `message` is considered safe to return to clients for 4xx errors. For
/// 5xx errors the message is redacted in the response body and only the generic
/// [`INTERNAL_ERROR_MESSAGE`] is sent, while the original detail and optional
/// cause are logged server-side.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    /// Explicit HTTP status, set by [`Error::http`]. Overrides `kind.status()`
    /// for the response while `kind` still supplies the default machine code.
    status: Option<StatusCode>,
    code: Option<&'static str>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// Concrete type of `source`, recorded so a typed exception handler can be
    /// located and the source downcast back to it (see [`Error::take_source`]).
    source_type: Option<TypeId>,
    details: Vec<ErrorDetail>,
}

/// A single field-level error, included in validation responses.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    /// Dotted path to the offending field.
    pub field: String,
    /// Machine-readable code describing what went wrong (for example
    /// `TOO_SHORT`).
    pub issue: String,
    /// Human-readable description of the problem.
    pub message: String,
}

impl ErrorDetail {
    /// Creates a field-level error detail.
    pub fn new(
        field: impl Into<String>,
        issue: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            issue: issue.into(),
            message: message.into(),
        }
    }
}

impl Error {
    /// Creates an error of the given kind with a client-facing message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            code: None,
            message: message.into(),
            source: None,
            source_type: None,
            details: Vec::new(),
        }
    }

    /// Creates an error with an explicit HTTP status code.
    ///
    /// Use this for a status outside the named constructors (for example `418`).
    /// The status sets the response status and reason phrase; the machine `code`
    /// defaults from the nearest [`ErrorKind`] and can be overridden with
    /// [`Error::with_code`]. An out-of-range value falls back to `500`.
    ///
    /// ```
    /// # use tork_core::Error;
    /// let error = Error::http(418, "I'm a teapot").with_code("TEAPOT");
    /// ```
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let kind = ErrorKind::from_status(status).unwrap_or(if status.is_server_error() {
            ErrorKind::Internal
        } else {
            ErrorKind::BadRequest
        });
        let mut error = Self::new(kind, message);
        error.status = Some(status);
        error
    }

    /// Creates a `400 Bad Request` error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadRequest, message)
    }

    /// Creates a `401 Unauthorized` error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unauthorized, message)
    }

    /// Creates a `403 Forbidden` error.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Forbidden, message)
    }

    /// Creates a `404 Not Found` error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// Creates a `405 Method Not Allowed` error.
    pub fn method_not_allowed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::MethodNotAllowed, message)
    }

    /// Creates a `409 Conflict` error.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, message)
    }

    /// Creates a `422 Unprocessable Entity` error.
    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unprocessable, message)
    }

    /// Creates a `413 Payload Too Large` error.
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::PayloadTooLarge, message)
    }

    /// Creates a `429 Too Many Requests` error.
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::TooManyRequests, message)
    }

    /// Creates a `500 Internal Server Error`.
    ///
    /// The message is logged but never returned to the client.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// Creates a `503 Service Unavailable` error.
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ServiceUnavailable, message)
    }

    /// Creates a `504 Gateway Timeout` error.
    pub fn gateway_timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::GatewayTimeout, message)
    }

    /// Overrides the machine-readable code (otherwise derived from the kind).
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// Attaches an underlying error as the cause, for server-side diagnostics.
    ///
    /// The cause is logged for server errors but is never serialized into a
    /// response body. Its concrete type is recorded so that a typed
    /// [`exception_handler`](crate::App::exception_handler) for `E` can be located
    /// and the cause recovered via [`take_source`](Error::take_source).
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self.source_type = Some(TypeId::of::<E>());
        self
    }

    /// Attaches field-level details, surfaced in the response body for `4xx`.
    pub fn with_details(mut self, details: Vec<ErrorDetail>) -> Self {
        self.details = details;
        self
    }

    /// Builds a validation error from a `garde` report.
    ///
    /// The code is set to `VALIDATION_FAILED` and each reported field path and
    /// message becomes an [`ErrorDetail`], with the issue classified from the
    /// message on a best-effort basis (garde does not expose structured codes).
    pub fn from_garde_report(report: garde::error::Report) -> Self {
        let details = report
            .iter()
            .map(|(path, error)| {
                let message = error.to_string();
                ErrorDetail::new(path.to_string(), classify_issue(&message), message)
            })
            .collect();
        Self::unprocessable(VALIDATION_ERROR_MESSAGE)
            .with_code(VALIDATION_ERROR_CODE)
            .with_details(details)
    }

    /// Returns the error category.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the machine-readable code (override, otherwise from the kind).
    pub fn code(&self) -> &str {
        self.code.unwrap_or_else(|| self.kind.code())
    }

    /// Returns the machine-readable code as a `'static` string.
    ///
    /// The code is always either an override or a kind default, both of which are
    /// `'static`; this lets a hook event hold the code without borrowing.
    pub(crate) fn static_code(&self) -> &'static str {
        self.code.unwrap_or_else(|| self.kind.code())
    }

    /// Returns the field-level details, if any.
    pub fn details(&self) -> &[ErrorDetail] {
        &self.details
    }

    /// Returns the client-facing message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the concrete type of the attached source, if any.
    ///
    /// Used to look up a typed exception handler registered for that type.
    pub(crate) fn source_type(&self) -> Option<TypeId> {
        self.source_type
    }

    /// Reports whether this is a request-body validation failure.
    pub(crate) fn is_validation(&self) -> bool {
        self.code() == VALIDATION_ERROR_CODE
    }

    /// Removes the attached source and returns it as `E`, if its concrete type
    /// matches.
    ///
    /// Returns `None` (leaving the source in place) when no source is attached or
    /// its type differs. This is how a typed
    /// [`exception_handler`](crate::App::exception_handler) recovers the original
    /// error value it was registered for.
    pub fn take_source<E>(&mut self) -> Option<E>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        if self.source_type != Some(TypeId::of::<E>()) {
            return None;
        }
        let source = self.source.take()?;
        self.source_type = None;
        match source.downcast::<E>() {
            Ok(typed) => Some(*typed),
            Err(restored) => {
                // Type id matched but the downcast did not: restore and bail.
                self.source = Some(restored);
                self.source_type = Some(TypeId::of::<E>());
                None
            }
        }
    }
}

/// Generates a named constructor for each remaining standard HTTP error status.
///
/// Each builds on [`Error::http`] (so the explicit status drives the response)
/// and pins the conventional machine `code`. The status-specific kinds with
/// distinct framework behavior (the ones above) are written by hand instead.
macro_rules! http_status_constructors {
    ($($name:ident, $code:literal, $machine:literal, $reason:literal;)*) => {
        impl Error {
            $(
                #[doc = concat!("Creates a `", stringify!($code), " ", $reason, "` error.")]
                pub fn $name(message: impl Into<String>) -> Self {
                    Self::http($code, message).with_code($machine)
                }
            )*
        }
    };
}

http_status_constructors! {
    payment_required, 402, "PAYMENT_REQUIRED", "Payment Required";
    not_acceptable, 406, "NOT_ACCEPTABLE", "Not Acceptable";
    proxy_authentication_required, 407, "PROXY_AUTHENTICATION_REQUIRED", "Proxy Authentication Required";
    request_timeout, 408, "REQUEST_TIMEOUT", "Request Timeout";
    gone, 410, "GONE", "Gone";
    length_required, 411, "LENGTH_REQUIRED", "Length Required";
    precondition_failed, 412, "PRECONDITION_FAILED", "Precondition Failed";
    uri_too_long, 414, "URI_TOO_LONG", "URI Too Long";
    unsupported_media_type, 415, "UNSUPPORTED_MEDIA_TYPE", "Unsupported Media Type";
    range_not_satisfiable, 416, "RANGE_NOT_SATISFIABLE", "Range Not Satisfiable";
    expectation_failed, 417, "EXPECTATION_FAILED", "Expectation Failed";
    im_a_teapot, 418, "IM_A_TEAPOT", "I'm a teapot";
    misdirected_request, 421, "MISDIRECTED_REQUEST", "Misdirected Request";
    locked, 423, "LOCKED", "Locked";
    failed_dependency, 424, "FAILED_DEPENDENCY", "Failed Dependency";
    too_early, 425, "TOO_EARLY", "Too Early";
    upgrade_required, 426, "UPGRADE_REQUIRED", "Upgrade Required";
    precondition_required, 428, "PRECONDITION_REQUIRED", "Precondition Required";
    request_header_fields_too_large, 431, "REQUEST_HEADER_FIELDS_TOO_LARGE", "Request Header Fields Too Large";
    unavailable_for_legal_reasons, 451, "UNAVAILABLE_FOR_LEGAL_REASONS", "Unavailable For Legal Reasons";
    not_implemented, 501, "NOT_IMPLEMENTED", "Not Implemented";
    bad_gateway, 502, "BAD_GATEWAY", "Bad Gateway";
    http_version_not_supported, 505, "HTTP_VERSION_NOT_SUPPORTED", "HTTP Version Not Supported";
    variant_also_negotiates, 506, "VARIANT_ALSO_NEGOTIATES", "Variant Also Negotiates";
    insufficient_storage, 507, "INSUFFICIENT_STORAGE", "Insufficient Storage";
    loop_detected, 508, "LOOP_DETECTED", "Loop Detected";
    not_extended, 510, "NOT_EXTENDED", "Not Extended";
    network_authentication_required, 511, "NETWORK_AUTHENTICATION_REQUIRED", "Network Authentication Required";
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Client-facing JSON body for an error.
#[derive(Serialize)]
struct ErrorBody<'a> {
    status: u16,
    code: &'a str,
    title: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "slice_is_empty")]
    details: &'a [ErrorDetail],
    #[serde(rename = "traceId")]
    trace_id: &'a str,
    timestamp: String,
}

/// Skips the `details` field when there are no field-level errors.
fn slice_is_empty(details: &&[ErrorDetail]) -> bool {
    details.is_empty()
}

/// Last-resort body used only if serializing the error body itself fails.
const FALLBACK_ERROR_BODY: &[u8] = br#"{"status":500,"code":"INTERNAL_SERVER_ERROR","title":"Internal Server Error","message":"Internal server error"}"#;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status.unwrap_or_else(|| self.kind.status());
        let trace_id = generate_trace_id();

        // Never leak internal detail on server errors: log the real cause (with
        // the trace id, so logs correlate with the client's response) and return
        // only the generic message.
        let message: &str = if status.is_server_error() {
            log_server_error(&self, &trace_id);
            INTERNAL_ERROR_MESSAGE
        } else {
            &self.message
        };

        // Field-level details are only surfaced for client errors.
        let details: &[ErrorDetail] = if status.is_server_error() {
            &[]
        } else {
            &self.details
        };

        let body = ErrorBody {
            status: status.as_u16(),
            code: self.code(),
            title: status.canonical_reason().unwrap_or("Error"),
            message,
            details,
            trace_id: &trace_id,
            timestamp: now_rfc3339(),
        };

        let mut response = match serde_json::to_vec(&body) {
            Ok(buffer) => with_body(status, APPLICATION_JSON, Bytes::from(buffer)),
            Err(_) => with_body(
                status,
                APPLICATION_JSON,
                Bytes::from_static(FALLBACK_ERROR_BODY),
            ),
        };
        // Keep proxies and browsers from caching error responses (a cached `401`
        // would block legitimate retries; a cached `500` would mask recovery).
        response.headers_mut().insert(
            http::header::CACHE_CONTROL,
            http::HeaderValue::from_static("no-store"),
        );
        response
    }
}

/// Generates a unique trace identifier for an error response.
///
/// The same identifier is logged for server errors, so a client-reported
/// `traceId` can be matched against server logs.
fn generate_trace_id() -> String {
    format!("{TRACE_ID_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Returns the current UTC time as an RFC 3339 timestamp with second precision.
fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|stamp| stamp.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// Classifies a `garde` validation message into a coarse issue code.
///
/// This is best effort: `garde` reports human messages, not structured codes, so
/// only well-known wordings are recognized; anything else maps to a generic code.
fn classify_issue(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("email") {
        "INVALID_FORMAT"
    } else if lower.contains("length is lower") {
        "TOO_SHORT"
    } else if lower.contains("length is greater") {
        "TOO_LONG"
    } else if lower.contains("must be greater than") {
        "TOO_SMALL"
    } else if lower.contains("must be less than") {
        "TOO_LARGE"
    } else if lower.contains("lower than") {
        "TOO_SMALL"
    } else if lower.contains("greater than") {
        "TOO_LARGE"
    } else {
        GENERIC_ISSUE
    }
}

/// Writes the full detail of a server error to standard error.
///
/// This is the framework's minimal default sink for server-side error detail; a
/// pluggable logging hook is planned for a later phase.
fn log_server_error(error: &Error, trace_id: &str) {
    match &error.source {
        Some(source) => eprintln!(
            "tork: server error [{trace_id}]: {}: {} (cause: {source})",
            error.code(),
            error.message,
        ),
        None => eprintln!(
            "tork: server error [{trace_id}]: {}: {}",
            error.code(),
            error.message,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::Response;
    use http_body_util::BodyExt;
    use serde_json::Value;

    async fn body_json(response: Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn status_mapping_matches_kind() {
        assert_eq!(ErrorKind::Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(ErrorKind::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            ErrorKind::Internal.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            ErrorKind::PayloadTooLarge.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            ErrorKind::GatewayTimeout.status(),
            StatusCode::GATEWAY_TIMEOUT
        );
    }

    #[tokio::test]
    async fn client_error_uses_problem_format() {
        let response = Error::forbidden("Access denied").into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = body_json(response).await;
        assert_eq!(body["status"], 403);
        assert_eq!(body["code"], "FORBIDDEN");
        assert_eq!(body["title"], "Forbidden");
        assert_eq!(body["message"], "Access denied");
        assert!(body.get("details").is_none(), "no details expected: {body}");
        assert!(
            body["traceId"].as_str().unwrap().starts_with("req-"),
            "traceId expected: {body}"
        );
        assert!(
            body["timestamp"].as_str().unwrap().ends_with('Z'),
            "timestamp: {body}"
        );
    }

    #[tokio::test]
    async fn server_error_is_redacted() {
        let response = Error::internal("database password is hunter2").into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = body_json(response).await;
        assert_eq!(body["code"], "INTERNAL_SERVER_ERROR");
        assert_eq!(body["message"], INTERNAL_ERROR_MESSAGE);
        assert!(
            !serde_json::to_string(&body).unwrap().contains("hunter2"),
            "internal detail must not leak"
        );
        // A trace id is still present so the operator can correlate logs.
        assert!(body["traceId"].as_str().unwrap().starts_with("req-"));
    }

    #[tokio::test]
    async fn http_uses_an_explicit_status_with_a_derived_title() {
        let response = Error::http(418, "I'm a teapot").into_response();
        assert_eq!(response.status(), StatusCode::from_u16(418).unwrap());

        let body = body_json(response).await;
        assert_eq!(body["status"], 418);
        assert_eq!(body["title"], "I'm a teapot");
        assert_eq!(body["message"], "I'm a teapot");
    }

    #[tokio::test]
    async fn http_matches_a_known_kind_code_and_allows_override() {
        // A known status reuses that kind's machine code.
        let response = Error::http(404, "gone").into_response();
        let body = body_json(response).await;
        assert_eq!(body["code"], "NOT_FOUND");

        // An explicit code still wins.
        let response = Error::http(418, "teapot").with_code("TEAPOT").into_response();
        let body = body_json(response).await;
        assert_eq!(body["code"], "TEAPOT");
    }

    #[tokio::test]
    async fn extended_status_constructors_set_status_code_and_title() {
        let cases = [
            (Error::im_a_teapot("no coffee"), 418, "IM_A_TEAPOT", "I'm a teapot"),
            (
                Error::unavailable_for_legal_reasons("blocked"),
                451,
                "UNAVAILABLE_FOR_LEGAL_REASONS",
                "Unavailable For Legal Reasons",
            ),
            (Error::too_early("retry later"), 425, "TOO_EARLY", "Too Early"),
            (Error::bad_gateway("upstream"), 502, "BAD_GATEWAY", "Bad Gateway"),
        ];
        for (error, status, code, title) in cases {
            let is_server = status >= 500;
            let response = error.into_response();
            assert_eq!(response.status().as_u16(), status);
            let body = body_json(response).await;
            assert_eq!(body["status"], status);
            assert_eq!(body["code"], code);
            // Server errors redact the message but still carry the right status/title.
            if !is_server {
                assert_eq!(body["title"], title);
            }
        }
    }

    #[tokio::test]
    async fn http_server_status_is_redacted() {
        let response = Error::http(503, "upstream pool exhausted: secret-host").into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["message"], INTERNAL_ERROR_MESSAGE);
        assert!(!serde_json::to_string(&body).unwrap().contains("secret-host"));
    }

    #[tokio::test]
    async fn validation_details_are_serialized() {
        let response = Error::unprocessable(VALIDATION_ERROR_MESSAGE)
            .with_code(VALIDATION_ERROR_CODE)
            .with_details(vec![ErrorDetail::new(
                "price",
                "TOO_SMALL",
                "must be greater than 0",
            )])
            .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = body_json(response).await;
        assert_eq!(body["code"], "VALIDATION_ERROR");
        assert_eq!(body["details"][0]["field"], "price");
        assert_eq!(body["details"][0]["issue"], "TOO_SMALL");
        assert_eq!(body["details"][0]["message"], "must be greater than 0");
    }

    #[derive(Debug, PartialEq)]
    struct SampleCause(&'static str);
    impl std::fmt::Display for SampleCause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for SampleCause {}

    #[derive(Debug)]
    struct OtherCause;
    impl std::fmt::Display for OtherCause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("other")
        }
    }
    impl std::error::Error for OtherCause {}

    #[test]
    fn with_source_records_the_type() {
        let error = Error::internal("boom").with_source(SampleCause("cause"));
        assert_eq!(error.source_type, Some(TypeId::of::<SampleCause>()));
    }

    #[test]
    fn take_source_round_trips_the_typed_cause() {
        let mut error = Error::internal("boom").with_source(SampleCause("cause"));
        assert_eq!(
            error.take_source::<SampleCause>(),
            Some(SampleCause("cause"))
        );
        // The source is consumed: a second take yields nothing.
        assert_eq!(error.take_source::<SampleCause>(), None);
        assert_eq!(error.source_type, None);
    }

    #[test]
    fn take_source_rejects_a_mismatched_type() {
        let mut error = Error::internal("boom").with_source(SampleCause("cause"));
        assert!(error.take_source::<OtherCause>().is_none());
        // The original source is left intact for the correct type.
        assert_eq!(error.source_type, Some(TypeId::of::<SampleCause>()));
        assert_eq!(
            error.take_source::<SampleCause>(),
            Some(SampleCause("cause"))
        );
    }

    #[test]
    fn take_source_is_none_without_a_source() {
        let mut error = Error::internal("boom");
        assert!(error.take_source::<SampleCause>().is_none());
    }

    #[test]
    fn from_garde_report_classifies_field_errors() {
        use garde::Validate;

        #[derive(garde::Validate)]
        struct Sample {
            #[garde(length(min = 3))]
            name: String,
        }

        let report = Sample {
            name: String::new(),
        }
        .validate()
        .unwrap_err();
        let error = Error::from_garde_report(report);

        assert_eq!(error.code(), "VALIDATION_ERROR");
        assert_eq!(error.details().len(), 1);
        assert_eq!(error.details()[0].field, "name");
        assert_eq!(error.details()[0].issue, "TOO_SHORT");
    }

    #[test]
    fn status_mapping_covers_every_kind() {
        use ErrorKind::*;
        assert_eq!(BadRequest.status(), StatusCode::BAD_REQUEST);
        assert_eq!(Unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(MethodNotAllowed.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(Conflict.status(), StatusCode::CONFLICT);
        assert_eq!(Unprocessable.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(PayloadTooLarge.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(TooManyRequests.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(Internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(ServiceUnavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(GatewayTimeout.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn code_mapping_covers_every_kind() {
        use ErrorKind::*;
        assert_eq!(BadRequest.code(), "BAD_REQUEST");
        assert_eq!(Unauthorized.code(), "UNAUTHORIZED");
        assert_eq!(Forbidden.code(), "FORBIDDEN");
        assert_eq!(NotFound.code(), "NOT_FOUND");
        assert_eq!(MethodNotAllowed.code(), "METHOD_NOT_ALLOWED");
        assert_eq!(Conflict.code(), "CONFLICT");
        assert_eq!(Unprocessable.code(), "UNPROCESSABLE_ENTITY");
        assert_eq!(PayloadTooLarge.code(), "PAYLOAD_TOO_LARGE");
        assert_eq!(TooManyRequests.code(), "TOO_MANY_REQUESTS");
        assert_eq!(Internal.code(), "INTERNAL_SERVER_ERROR");
        assert_eq!(ServiceUnavailable.code(), "SERVICE_UNAVAILABLE");
        assert_eq!(GatewayTimeout.code(), "GATEWAY_TIMEOUT");
    }

    #[test]
    fn method_not_allowed_constructor_uses_method_not_allowed_kind() {
        let error = Error::method_not_allowed("GET not allowed");
        assert_eq!(error.kind(), ErrorKind::MethodNotAllowed);
        assert_eq!(error.message(), "GET not allowed");
    }

    #[test]
    fn conflict_constructor_uses_conflict_kind() {
        let error = Error::conflict("duplicate key");
        assert_eq!(error.kind(), ErrorKind::Conflict);
        assert_eq!(error.message(), "duplicate key");
    }

    #[test]
    fn too_many_requests_constructor_uses_too_many_requests_kind() {
        let error = Error::too_many_requests("slow down");
        assert_eq!(error.kind(), ErrorKind::TooManyRequests);
        assert_eq!(error.message(), "slow down");
    }

    #[test]
    fn service_unavailable_constructor_uses_service_unavailable_kind() {
        let error = Error::service_unavailable("maintenance");
        assert_eq!(error.kind(), ErrorKind::ServiceUnavailable);
        assert_eq!(error.message(), "maintenance");
    }

    #[test]
    fn error_trait_source_returns_attached_source() {
        use std::error::Error as _;
        let error = Error::internal("boom").with_source(SampleCause("inner"));
        let source = error.source().expect("source should be present");
        assert_eq!(source.to_string(), "inner");
    }

    #[test]
    fn error_trait_source_is_none_when_unset() {
        use std::error::Error as _;
        let error = Error::internal("boom");
        assert!(error.source().is_none());
    }

    #[test]
    fn take_source_restores_state_when_downcast_defensively_fails() {
        // Simulate a state where the recorded TypeId points to SampleCause
        // but the boxed source is something else. This exercises the
        // defensive downcast-failure branch (line ~318 in source).
        let mut error = Error::internal("boom");
        error.source = Some(Box::new(OtherCause));
        error.source_type = Some(TypeId::of::<SampleCause>());

        // The take must return None and the state must be preserved.
        assert!(error.take_source::<SampleCause>().is_none());
        assert_eq!(error.source_type, Some(TypeId::of::<SampleCause>()));
    }

    #[test]
    fn sample_cause_display_formats_inner_message() {
        assert_eq!(SampleCause("payload").to_string(), "payload");
    }

    #[test]
    fn other_cause_display_formats_inner_message() {
        assert_eq!(OtherCause.to_string(), "other");
    }

    #[test]
    fn fallback_body_constant_is_valid_json() {
        let parsed: Value = serde_json::from_slice(FALLBACK_ERROR_BODY).unwrap();
        assert_eq!(parsed["status"], 500);
        assert_eq!(parsed["code"], "INTERNAL_SERVER_ERROR");
    }

    #[test]
    fn classify_issue_recognizes_email_format() {
        assert_eq!(classify_issue("email is not valid"), "INVALID_FORMAT");
        assert_eq!(classify_issue("Email is invalid"), "INVALID_FORMAT");
    }

    #[test]
    fn classify_issue_recognizes_too_long() {
        assert_eq!(classify_issue("length is greater than 10"), "TOO_LONG");
    }

    #[test]
    fn classify_issue_recognizes_strict_numeric_bounds() {
        assert_eq!(classify_issue("value must be greater than 0"), "TOO_SMALL");
        assert_eq!(classify_issue("value must be less than 100"), "TOO_LARGE");
    }

    #[test]
    fn classify_issue_falls_back_to_generic() {
        assert_eq!(classify_issue("something unrelated"), "INVALID");
        assert_eq!(classify_issue(""), "INVALID");
    }
}
