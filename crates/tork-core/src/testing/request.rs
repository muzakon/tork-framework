//! Request builders for the test client.

use std::sync::Arc;

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue, Method};
use serde::Serialize;

use super::client::Shared;
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
    headers: Vec<(HeaderName, HeaderValue)>,
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
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            self.headers.push((name, value));
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
    headers: Vec<(HeaderName, HeaderValue)>,
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
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            self.headers.push((name, value));
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
                        format!(
                            "Content-Disposition: form-data; name=\"{}\"\r\n",
                            part.name
                        )
                        .as_bytes(),
                    );
                }
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(&part.value);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

        let content_type =
            HeaderValue::from_str(&format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}"))
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
