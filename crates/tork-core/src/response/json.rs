//! JSON response support.

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use crate::constants::APPLICATION_JSON;
use crate::error::Error;
use crate::response::{IntoResponse, Response, with_body};

/// A JSON response wrapper.
///
/// Wrapping a serializable value in `Json` renders it as an `application/json`
/// response body with a `200 OK` status. Handlers usually return the bare model
/// and let the generated route glue serialize it; `Json` is available for when a
/// response body needs to be built explicitly.
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        json_response(StatusCode::OK, &self.0)
    }
}

/// Serializes `value` to JSON and builds a response with the given status.
///
/// # Errors
///
/// If serialization fails, a redacted `500 Internal Server Error` is returned so
/// that a partially written or malformed body is never sent to the client.
pub fn json_response<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(buffer) => with_body(status, APPLICATION_JSON, Bytes::from(buffer)),
        Err(error) => Error::internal("response body serialization failed")
            .with_source(error)
            .into_response(),
    }
}
