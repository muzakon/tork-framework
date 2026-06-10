//! Body type aliases shared across the runtime.

use bytes::Bytes;

/// The inbound request body type, as produced by Hyper.
pub type ReqBody = hyper::body::Incoming;

/// The outbound response body type.
///
/// Responses are fully buffered in this phase, so a single contiguous byte
/// buffer is sufficient and avoids the overhead of a boxed, dynamically typed
/// body. Streaming bodies can be introduced later behind the same [`Response`]
/// alias without changing handler signatures.
///
/// [`Response`]: crate::Response
pub type RespBody = http_body_util::Full<Bytes>;
