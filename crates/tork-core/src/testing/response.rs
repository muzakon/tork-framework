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
