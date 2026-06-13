//! Request builders for the test client.

use std::sync::Arc;

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue, Method};
use serde::Serialize;

use super::client::{Shared, TestHeader};
use super::response::TestResponse;
use crate::error::{Error, Result};

/// Boundary used for generated multipart bodies. Unlikely to occur in test data.
const MULTIPART_BOUNDARY: &str = "----TorkTestBoundary7MA4YWxkTrZu0gW";

/// A pending request body and its content type.
#[derive(Default)]
pub(crate) struct PendingBody {
    pub(crate) content_type: Option<HeaderValue>,
    pub(crate) bytes: Bytes,
}

/// Builds and sends a single HTTP request.
///
/// Created by the verb methods on [`TestClient`](super::TestClient)
/// (`get`/`post`/...). Set headers, query parameters, and a body, then call
/// [`send`](TestRequestBuilder::send).
pub struct TestRequestBuilder {
    shared: Arc<Shared>,
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<TestHeader>,
    body: PendingBody,
}

impl TestRequestBuilder {
    pub(crate) fn new(shared: Arc<Shared>, method: Method, path: impl Into<String>) -> Self {
        Self {
            shared,
            method,
            path: path.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: PendingBody::default(),
        }
    }

    /// Adds a request header. An invalid name or value is ignored.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::safe(name, value));
        }
        self
    }

    /// Adds a security-sensitive request header, bypassing the in-process guard.
    pub fn unsafe_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::unsafe_allowed(name, value));
        }
        self
    }

    /// Adds a query parameter.
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Sets a JSON body (`application/json`).
    pub fn json<T: Serialize>(mut self, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                self.body = PendingBody {
                    content_type: Some(HeaderValue::from_static("application/json")),
                    bytes: Bytes::from(bytes),
                };
            }
            Err(_) => self.body = PendingBody::default(),
        }
        self
    }

    /// Sets a urlencoded form body (`application/x-www-form-urlencoded`).
    pub fn form<T: Serialize>(mut self, value: &T) -> Self {
        match serde_urlencoded::to_string(value) {
            Ok(text) => {
                self.body = PendingBody {
                    content_type: Some(HeaderValue::from_static(
                        "application/x-www-form-urlencoded",
                    )),
                    bytes: Bytes::from(text.into_bytes()),
                };
            }
            Err(_) => self.body = PendingBody::default(),
        }
        self
    }

    /// Sets a raw byte body, with no content type unless one is set via `header`.
    pub fn bytes(mut self, bytes: impl Into<Bytes>) -> Self {
        self.body = PendingBody {
            content_type: None,
            bytes: bytes.into(),
        };
        self
    }

    /// Switches to a `multipart/form-data` body builder.
    pub fn multipart(self) -> TestMultipartBuilder {
        TestMultipartBuilder {
            shared: self.shared,
            method: self.method,
            path: self.path,
            query: self.query,
            headers: self.headers,
            parts: Vec::new(),
        }
    }

    /// Sends the request and returns the response.
    pub async fn send(self) -> Result<TestResponse> {
        self.shared
            .send(self.method, self.path, self.query, self.headers, self.body)
            .await
    }

    /// Sends the request and reads the response as a Server-Sent Events stream.
    pub async fn sse(self) -> Result<super::sse::TestSseStream> {
        self.shared
            .open_sse(self.method, self.path, self.query, self.headers)
            .await
    }
}

/// One part of a multipart body: a text field or a file.
struct MultipartPart {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    value: Bytes,
}

/// Builds and sends a `multipart/form-data` request (forms with files).
pub struct TestMultipartBuilder {
    shared: Arc<Shared>,
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<TestHeader>,
    parts: Vec<MultipartPart>,
}

impl TestMultipartBuilder {
    /// Adds a text field.
    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.parts.push(MultipartPart {
            name: name.to_owned(),
            filename: None,
            content_type: None,
            value: Bytes::from(value.to_owned().into_bytes()),
        });
        self
    }

    /// Adds a file field with the given filename, content type, and bytes.
    pub fn file_bytes(
        mut self,
        name: &str,
        filename: &str,
        content_type: &str,
        bytes: impl Into<Bytes>,
    ) -> Self {
        self.parts.push(MultipartPart {
            name: name.to_owned(),
            filename: Some(filename.to_owned()),
            content_type: Some(content_type.to_owned()),
            value: bytes.into(),
        });
        self
    }

    /// Adds a request header.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::safe(name, value));
        }
        self
    }

    /// Adds a security-sensitive request header, bypassing the in-process guard.
    pub fn unsafe_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::unsafe_allowed(name, value));
        }
        self
    }

    /// Adds a query parameter.
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Encodes the parts and sends the request.
    pub async fn send(self) -> Result<TestResponse> {
        let mut body = Vec::new();
        for part in &self.parts {
            body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}\r\n").as_bytes());
            match (&part.filename, &part.content_type) {
                (Some(filename), content_type) => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                            part.name, filename
                        )
                        .as_bytes(),
                    );
                    if let Some(content_type) = content_type {
                        body.extend_from_slice(
                            format!("Content-Type: {content_type}\r\n").as_bytes(),
                        );
                    }
                }
                (None, _) => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{}\"\r\n", part.name)
                            .as_bytes(),
                    );
                }
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(&part.value);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

        let content_type = HeaderValue::from_str(&format!(
            "multipart/form-data; boundary={MULTIPART_BOUNDARY}"
        ))
        .map_err(|_| Error::internal("failed to build multipart content type"))?;
        let pending = PendingBody {
            content_type: Some(content_type),
            bytes: Bytes::from(body),
        };
        self.shared
            .send(self.method, self.path, self.query, self.headers, pending)
            .await
    }
}

/// The `Content-Type` header name, re-exported for the client module.
pub(crate) const CONTENT_TYPE_HEADER: HeaderName = CONTENT_TYPE;

#[cfg(test)]
mod tests {
    use super::super::client::{Shared, Transport};
    use super::*;
    use crate::app::App;
    use std::sync::Mutex;

    #[derive(serde::Serialize)]
    struct Query {
        word: &'static str,
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

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            transport: Transport::InProcess(Arc::new(App::new().build().unwrap())),
            default_headers: http::HeaderMap::new(),
            unsafe_default_headers: http::HeaderMap::new(),
            cookies: Mutex::new(super::super::cookie::CookieJar::default()),
        })
    }

    #[test]
    fn json_and_form_reset_body_on_serialize_failure() {
        let request = TestRequestBuilder::new(shared(), Method::POST, "/items")
            .json(&BrokenSerialize)
            .form(&BrokenSerialize);

        assert!(request.body.content_type.is_none());
        assert!(request.body.bytes.is_empty());
    }

    #[test]
    fn builder_collects_headers_query_and_bytes() {
        let request = TestRequestBuilder::new(shared(), Method::PUT, "/items")
            .header("x-test", "1")
            .header("\n", "ignored")
            .query("q", "space value")
            .bytes(Bytes::from_static(b"payload"));

        assert_eq!(request.headers.len(), 1);
        assert_eq!(
            request.query,
            vec![("q".to_owned(), "space value".to_owned())]
        );
        assert_eq!(request.body.bytes, Bytes::from_static(b"payload"));
        assert!(request.body.content_type.is_none());
        assert!(!request.headers[0].unsafe_allowed);
    }

    #[test]
    fn unsafe_header_marks_the_entry() {
        let request = TestRequestBuilder::new(shared(), Method::GET, "/items")
            .unsafe_header("host", "example.com");

        assert_eq!(request.headers.len(), 1);
        assert!(request.headers[0].unsafe_allowed);
    }

    #[tokio::test]
    async fn multipart_builder_encodes_text_and_file_parts() {
        let response = TestRequestBuilder::new(shared(), Method::POST, "/upload")
            .multipart()
            .text("title", "hello")
            .file_bytes("file", "note.txt", "text/plain", "payload")
            .query("kind", "docs")
            .header("x-test", "1")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 404);
    }

    #[test]
    fn form_uses_urlencoding() {
        let request = TestRequestBuilder::new(shared(), Method::POST, "/search").form(&Query {
            word: "hello world",
        });

        assert_eq!(
            request.body.content_type,
            Some(HeaderValue::from_static(
                "application/x-www-form-urlencoded"
            ))
        );
        assert_eq!(request.body.bytes, Bytes::from_static(b"word=hello+world"));
    }
}
