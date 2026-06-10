//! Streamed event model.

use tork::api_model;

/// A single event delivered over the Server-Sent Events stream.
#[api_model(rename_all = "camelCase")]
pub struct EventOut {
    /// Monotonic sequence number, usable as the SSE event id.
    pub sequence: i64,
    /// Human-readable event message.
    pub message: String,
}
