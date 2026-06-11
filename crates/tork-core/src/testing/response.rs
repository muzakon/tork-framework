//! The response returned by the test client.

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// A buffered response captured by the [`TestClient`](super::TestClient).
///
/// The body is fully read when the request is sent, so the accessors are cheap and
/// can be called repeatedly.
pub struct TestResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

impl TestResponse {
    /// Returns the status code as a number.
    pub fn status(&self) -> u16 {
        self.status.as_u16()
    }

    /// Returns the status code.
    pub fn status_code(&self) -> StatusCode {
        self.status
    }

    /// Returns the response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns the raw response body.
    pub fn bytes(&self) -> Bytes {
        self.body.clone()
    }

    /// Returns the response body as UTF-8 text.
    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.to_vec())
            .map_err(|_| Error::internal("response body is not valid UTF-8"))
    }

    /// Deserializes the response body as JSON.
    pub async fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|error| Error::internal(format!("response body is not valid JSON: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http::HeaderValue;
    use serde::Deserialize;

    fn response(body: &[u8]) -> TestResponse {
        let mut headers = HeaderMap::new();
        headers.insert("x-test", HeaderValue::from_static("ok"));
        TestResponse {
            status: StatusCode::CREATED,
            headers,
            body: Bytes::copy_from_slice(body),
        }
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Payload {
        ok: bool,
    }

    #[tokio::test]
    async fn exposes_status_headers_bytes_and_json() {
        let response = response(br#"{"ok":true}"#);

        assert_eq!(response.status(), 201);
        assert_eq!(response.status_code(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-test"], "ok");
        assert_eq!(response.bytes(), Bytes::from_static(br#"{"ok":true}"#));
        assert_eq!(response.text().unwrap(), r#"{"ok":true}"#);
        assert_eq!(response.json::<Payload>().await.unwrap(), Payload { ok: true });
    }

    #[test]
    fn text_rejects_invalid_utf8() {
        let response = response(&[0xff, 0xfe]);

        let error = response.text().unwrap_err();
        assert_eq!(error.message(), "response body is not valid UTF-8");
    }

    #[tokio::test]
    async fn json_rejects_invalid_payload() {
        let response = response(br#"{"ok":"wrong"}"#);

        let error = response.json::<Payload>().await.unwrap_err();
        assert!(error.message().starts_with("response body is not valid JSON:"));
    }
}
