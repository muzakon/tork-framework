//! JSON response support.

use bytes::Bytes;
use http::StatusCode;
use serde::Serialize;

use crate::constants::APPLICATION_JSON;
use crate::error::Error;
use crate::response::{with_body, IntoResponse, Response};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::into_body_bytes;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Payload {
        ok: bool,
    }

    struct BrokenSerialize;

    impl Serialize for BrokenSerialize {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("boom"))
        }
    }

    #[tokio::test]
    async fn json_response_serializes_payload() {
        let response = json_response(StatusCode::CREATED, &Payload { ok: true });
        let (parts, body) = into_body_bytes(response).await;

        assert_eq!(parts.status, StatusCode::CREATED);
        assert_eq!(parts.headers["content-type"], APPLICATION_JSON);
        assert_eq!(body, Bytes::from_static(br#"{"ok":true}"#));
    }

    #[tokio::test]
    async fn json_response_redacts_serialize_failures() {
        let response = json_response(StatusCode::OK, &BrokenSerialize);
        let (parts, body) = into_body_bytes(response).await;

        assert_eq!(parts.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(parts.headers["content-type"], APPLICATION_JSON);
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("INTERNAL_SERVER_ERROR"));
    }

    #[tokio::test]
    async fn json_response_does_not_leak_serialize_error_message() {
        let response = json_response(StatusCode::OK, &BrokenSerialize);
        let (_, body) = into_body_bytes(response).await;
        let body_str = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !body_str.contains("boom"),
            "serialization error message leaked into response body: {body_str}"
        );
    }

    #[tokio::test]
    async fn json_wrapper_serializes_payload_with_default_ok_status() {
        let response = Json(Payload { ok: true }).into_response();
        let (parts, body) = into_body_bytes(response).await;

        assert_eq!(parts.status, StatusCode::OK);
        assert_eq!(parts.headers["content-type"], APPLICATION_JSON);
        assert_eq!(body, Bytes::from_static(br#"{"ok":true}"#));
    }

    #[tokio::test]
    async fn json_wrapper_accepts_dynamic_json_value() {
        let value = serde_json::json!({ "name": "alice", "age": 30 });
        let response = Json(value).into_response();
        let (parts, body) = into_body_bytes(response).await;

        assert_eq!(parts.status, StatusCode::OK);
        assert_eq!(parts.headers["content-type"], APPLICATION_JSON);
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], "alice");
        assert_eq!(parsed["age"], 30);
    }

    #[tokio::test]
    async fn json_response_preserves_custom_status_code() {
        let response = json_response(StatusCode::ACCEPTED, &Payload { ok: false });
        let (parts, body) = into_body_bytes(response).await;

        assert_eq!(parts.status, StatusCode::ACCEPTED);
        assert_eq!(parts.headers["content-type"], APPLICATION_JSON);
        assert_eq!(body, Bytes::from_static(br#"{"ok":false}"#));
    }
}
