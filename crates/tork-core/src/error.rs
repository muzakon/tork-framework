//! Error type and HTTP error responses.

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use crate::constants::{APPLICATION_JSON, INTERNAL_ERROR_MESSAGE};
use crate::response::{IntoResponse, Response, with_body};

/// A specialized [`Result`](core::result::Result) whose error type defaults to
/// [`Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

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
            ErrorKind::Conflict => StatusCode::CONFLICT,
            ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Returns a stable, machine-readable code for this error category.
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Unprocessable => "unprocessable_entity",
            ErrorKind::TooManyRequests => "too_many_requests",
            ErrorKind::Internal => "internal_error",
            ErrorKind::ServiceUnavailable => "service_unavailable",
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
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Creates an error of the given kind with a client-facing message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
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

    /// Returns the error category.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the client-facing message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.code(), self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Client-facing JSON shape for an error: `{"error": {"code", "message"}}`.
#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

/// Last-resort body used only if serializing the error envelope itself fails.
const FALLBACK_ERROR_BODY: &[u8] =
    br#"{"error":{"code":"internal_error","message":"Internal server error"}}"#;

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.kind.status();

        // Never leak internal detail on server errors: log the real cause and
        // return only the generic message in the response body.
        let message: &str = if status.is_server_error() {
            log_server_error(&self);
            INTERNAL_ERROR_MESSAGE
        } else {
            &self.message
        };

        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.kind.code(),
                message,
            },
        };

        match serde_json::to_vec(&envelope) {
            Ok(buffer) => with_body(status, APPLICATION_JSON, Bytes::from(buffer)),
            Err(_) => with_body(status, APPLICATION_JSON, Bytes::from_static(FALLBACK_ERROR_BODY)),
        }
    }
}

/// Writes the full detail of a server error to standard error.
///
/// This is the framework's minimal default sink for server-side error detail; a
/// pluggable logging hook is planned for a later phase.
fn log_server_error(error: &Error) {
    match &error.source {
        Some(source) => eprintln!(
            "tork: server error: {}: {} (cause: {source})",
            error.kind.code(),
            error.message,
        ),
        None => eprintln!(
            "tork: server error: {}: {}",
            error.kind.code(),
            error.message,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::Response;
    use http_body_util::BodyExt;

    async fn body_to_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn status_mapping_matches_kind() {
        assert_eq!(ErrorKind::Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(ErrorKind::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(ErrorKind::Internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn client_error_keeps_message() {
        let response = Error::forbidden("Access denied").into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = body_to_string(response).await;
        assert!(body.contains("Access denied"), "client message should be visible");
        assert!(body.contains("forbidden"), "error code should be present");
    }

    #[tokio::test]
    async fn server_error_is_redacted() {
        let response = Error::internal("database password is hunter2").into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = body_to_string(response).await;
        assert!(!body.contains("hunter2"), "internal detail must not leak");
        assert!(body.contains(INTERNAL_ERROR_MESSAGE), "generic message expected");
    }
}
