//! Health-check model.

use tork::api_model;

/// Health-check response.
#[api_model]
pub struct HealthOut {
    /// Liveness status.
    pub status: String,
    /// Service name, taken from configuration.
    pub service: String,
}
