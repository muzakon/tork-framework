//! Chat message models for the WebSocket demo.

use tork::api_model;

/// A message a client sends into a chat room.
#[api_model(rename_all = "camelCase")]
pub struct ChatIn {
    /// The message text; must not be empty.
    #[field(min_length = 1, max_length = 500)]
    pub message: String,
}

/// A message broadcast to everyone in a chat room.
#[api_model(rename_all = "camelCase")]
pub struct ChatMessage {
    /// Display name of the sender.
    pub from: String,
    /// The message text.
    pub text: String,
}
