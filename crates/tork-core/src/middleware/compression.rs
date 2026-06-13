//! Response compression middleware.

use std::io::Write;

use bytes::Bytes;
use flate2::Compression as CompressionLevel;
use flate2::write::GzEncoder;
use http::HeaderValue;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY};

use crate::body::RespBody;
use crate::constants::TEXT_EVENT_STREAM;
use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::{Response, into_body_bytes};
use crate::router::BoxFuture;

/// Content-coding token for gzip.
const GZIP: &str = "gzip";

/// Default upper bound on a body eligible for compression (8 MiB).
///
/// Compressing buffers the whole body in memory and produces a second buffer for
/// the gzip output, so a very large response can multiply peak memory per request.
/// Bodies above this size are passed through uncompressed; when a `Content-Length`
/// advertises the size, they are not even buffered.
const DEFAULT_MAXIMUM_SIZE: usize = 8 * 1024 * 1024;

/// Compresses response bodies when the client supports it.
///
/// When gzip is enabled, the client's `Accept-Encoding` includes gzip, the
/// response has no existing `Content-Encoding`, and the body is between
/// `minimum_size` and `maximum_size` bytes, the body is gzip-compressed and the
/// relevant headers are set.
pub struct Compression {
    gzip: bool,
    minimum_size: usize,
    maximum_size: usize,
}

impl Compression {
    /// Creates a compression middleware with no algorithm enabled yet.
    pub fn new() -> Self {
        Self {
            gzip: false,
            minimum_size: 0,
            maximum_size: DEFAULT_MAXIMUM_SIZE,
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

    /// Sets the maximum body size (in bytes) eligible for compression.
    ///
    /// Bodies larger than this are sent uncompressed to bound per-request memory;
    /// when the response advertises a `Content-Length` over this limit, the body is
    /// streamed through without being buffered. Use `usize::MAX` to lift the cap.
    pub fn maximum_size(mut self, bytes: usize) -> Self {
        self.maximum_size = bytes;
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
        let maximum_size = self.maximum_size;
        let accepts_gzip = request
            .headers()
            .get(ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_ascii_lowercase().contains(GZIP))
            .unwrap_or(false);

        Box::pin(async move {
            let mut response = next.run(request).await?;

            // When gzip is enabled the same URL may be served compressed or not
            // depending on the client's `Accept-Encoding`, so every eligible
            // response must carry `Vary: Accept-Encoding` (not just the compressed
            // one) or a cache could hand a compressed body to a client that did not
            // ask for it.
            if gzip_enabled && !is_event_stream(&response) {
                append_vary_accept_encoding(response.headers_mut());
            }

            // Skip when gzip is off, unsupported, the body is already encoded, or
            // the body is a stream (an event stream must not be buffered here, and
            // streaming responses are not worth compressing frame by frame).
            if !gzip_enabled
                || !accepts_gzip
                || response.headers().contains_key(CONTENT_ENCODING)
                || is_event_stream(&response)
            {
                return Ok(response);
            }

            // If the response advertises a length over the cap, or is already below
            // the minimum compression threshold, pass it through without buffering.
            if let Some(length) = content_length(response.headers()) {
                if length > maximum_size || length < minimum_size {
                    return Ok(response);
                }
            }

            let (mut parts, bytes) = into_body_bytes(response).await;
            // Out of the eligible window: too small to be worth it, or large enough
            // that compressing would add a second big buffer for little gain.
            if bytes.len() < minimum_size || bytes.len() > maximum_size {
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

/// Adds `Accept-Encoding` to the response's `Vary` header unless already listed.
fn append_vary_accept_encoding(headers: &mut http::HeaderMap) {
    let already_present = headers
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.to_ascii_lowercase().contains("accept-encoding"));
    if !already_present {
        headers.append(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

/// Parses the response's `Content-Length` header, if present and valid.
fn content_length(headers: &http::HeaderMap) -> Option<usize> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
}

/// Reports whether the response is a Server-Sent Events stream.
///
/// Such a body is unbounded and must not be buffered for compression.
fn is_event_stream(response: &Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with(TEXT_EVENT_STREAM))
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with_content_type(value: &'static str) -> Response {
        let mut response = http::Response::new(RespBody::new(Bytes::new()));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(value));
        response
    }

    #[test]
    fn event_stream_is_detected_and_bypasses_compression() {
        assert!(is_event_stream(&response_with_content_type(TEXT_EVENT_STREAM)));
        assert!(!is_event_stream(&response_with_content_type("application/json")));
        // A response without a content type is not treated as an event stream.
        assert!(!is_event_stream(&http::Response::new(RespBody::new(Bytes::new()))));
    }

    #[test]
    fn content_length_parses_only_valid_values() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(content_length(&headers), None);
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("1024"));
        assert_eq!(content_length(&headers), Some(1024));
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("not-a-number"));
        assert_eq!(content_length(&headers), None);
    }

    #[test]
    fn gzip_round_trips_through_flate2() {
        let original = b"hello world, this is a test that compresses well. ".repeat(20);
        let compressed = gzip(&original).expect("gzip must succeed");
        // Compressed data should be smaller than original (highly repetitive).
        assert!(compressed.len() < original.len());
    }
}
