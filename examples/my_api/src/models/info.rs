//! Configuration info model.

use tork::api_model;

/// A view of the loaded configuration, returned by `/demo/info`.
#[api_model(rename_all = "camelCase")]
pub struct InfoOut {
    /// The configured application name.
    pub app_name: String,
    /// The active environment.
    pub environment: String,
    /// The configured per-user item limit.
    pub items_per_user: u32,
    /// The address the server binds to.
    pub server_host: String,
    /// The port the server listens on.
    pub server_port: u16,
}
