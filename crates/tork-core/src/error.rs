//! Error type and HTTP error responses.

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use crate::constants::{APPLICATION_JSON, INTERNAL_ERROR_MESSAGE};
use crate::response::{IntoResponse, Response, with_body};

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
    /// `422 Unprocessable Entity`.
    Unprocessable,
    /// `429 Too Many Requests`.
    TooManyRequests,
    /// `500 Internal Server Error`.
    Internal,
    /// `503 Service Unavailable`.
    ServiceUnavailable,
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
            ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
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
            ErrorKind::Unprocessable => "UNPROCESSABLE_ENTITY",
            ErrorKind::TooManyRequests => "TOO_MANY_REQUESTS",
            ErrorKind::Internal => "INTERNAL_SERVER_ERROR",
            ErrorKind::ServiceUnavailable => "SERVICE_UNAVAILABLE",
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
    code: Option<&'static str>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
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
            code: None,
            message: message.into(),
            source: None,
            details: Vec::new(),
        }
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

    /// Overrides the machine-readable code (otherwise derived from the kind).
    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    /// Attaches an underlying error as the cause, for server-side diagnostics.
    ///
    /// The cause is logged for server errors but is never serialized into a
    /// response body.
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
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

    /// Returns the field-level details, if any.
    pub fn details(&self) -> &[ErrorDetail] {
        &self.details
    }

    /// Returns the client-facing message.
    pub fn message(&self) -> &str {
        &self.message
    }
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
        let status = self.kind.status();
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

        match serde_json::to_vec(&body) {
            Ok(buffer) => with_body(status, APPLICATION_JSON, Bytes::from(buffer)),
            Err(_) => with_body(status, APPLICATION_JSON, Bytes::from_static(FALLBACK_ERROR_BODY)),
        }
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
        assert_eq!(ErrorKind::Internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
        assert!(body["timestamp"].as_str().unwrap().ends_with('Z'), "timestamp: {body}");
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
}
