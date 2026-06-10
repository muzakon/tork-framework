//! Body type aliases shared across the runtime.

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;

/// Boxed, thread-safe error type carried by an erased request body.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The inbound request body type.
///
/// The concrete body produced by Hyper is erased into this boxed body so the
/// runtime is agnostic to where a request body comes from. This also makes the
/// request context easy to construct in tests.
pub type ReqBody = UnsyncBoxBody<Bytes, BoxError>;

/// The outbound response body type.
///
/// Responses are fully buffered in this phase, so a single contiguous byte
/// buffer is sufficient and avoids the overhead of a boxed, dynamically typed
/// body. Streaming bodies can be introduced later behind the same [`Response`]
/// alias without changing handler signatures.
///
/// [`Response`]: crate::Response
pub type RespBody = http_body_util::Full<Bytes>;

/// Erases any compatible HTTP body into the runtime's [`ReqBody`] type.
pub fn box_body<B>(body: B) -> ReqBody
where
    B: hyper::body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    body.map_err(Into::into).boxed_unsync()
}
