//! Response compression middleware.

use std::io::Write;

use bytes::Bytes;
use flate2::Compression as CompressionLevel;
use flate2::write::GzEncoder;
use http::HeaderValue;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, VARY};

use crate::body::RespBody;
use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::{Response, into_body_bytes};
use crate::router::BoxFuture;

/// Content-coding token for gzip.
const GZIP: &str = "gzip";

/// Compresses response bodies when the client supports it.
///
/// When gzip is enabled, the client's `Accept-Encoding` includes gzip, the
/// response has no existing `Content-Encoding`, and the body is at least
/// `minimum_size` bytes, the body is gzip-compressed and the relevant headers
/// are set.
pub struct Compression {
    gzip: bool,
    minimum_size: usize,
}

impl Compression {
    /// Creates a compression middleware with no algorithm enabled yet.
    pub fn new() -> Self {
        Self {
            gzip: false,
            minimum_size: 0,
        }
    }

    /// Enables gzip compression.
    pub fn gzip(mut self) -> Self {
        self.gzip = true;
        self
    }

    /// Sets the minimum body size (in bytes) eligible for compression.
    pub fn minimum_size(mut self, bytes: usize) -> Self {
        self.minimum_size = bytes;
        self
    }
}

impl Default for Compression {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for Compression {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let gzip_enabled = self.gzip;
        let minimum_size = self.minimum_size;
        let accepts_gzip = request
            .headers()
            .get(ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_ascii_lowercase().contains(GZIP))
            .unwrap_or(false);

        Box::pin(async move {
            let response = next.run(request).await?;

            // Skip when gzip is off, unsupported, or the body is already encoded.
            if !gzip_enabled || !accepts_gzip || response.headers().contains_key(CONTENT_ENCODING) {
                return Ok(response);
            }

            let (mut parts, bytes) = into_body_bytes(response).await;
            if bytes.len() < minimum_size {
                return Ok(Response::from_parts(parts, RespBody::new(bytes)));
            }

            match gzip(&bytes) {
                Ok(compressed) => {
                    parts
                        .headers
                        .insert(CONTENT_ENCODING, HeaderValue::from_static(GZIP));
                    if let Ok(length) = HeaderValue::from_str(&compressed.len().to_string()) {
                        parts.headers.insert(CONTENT_LENGTH, length);
                    }
                    parts
                        .headers
                        .append(VARY, HeaderValue::from_static("Accept-Encoding"));
                    Ok(Response::from_parts(
                        parts,
                        RespBody::new(Bytes::from(compressed)),
                    ))
                }
                // On the unlikely encode failure, send the body uncompressed.
                Err(_) => Ok(Response::from_parts(parts, RespBody::new(bytes))),
            }
        })
    }

    fn name(&self) -> &'static str {
        "Compression"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

/// Gzip-compresses a byte slice.
fn gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(
        Vec::with_capacity(data.len() / 2 + 16),
        CompressionLevel::default(),
    );
    encoder.write_all(data)?;
    encoder.finish()
}
