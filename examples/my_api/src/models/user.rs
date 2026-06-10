//! User-facing models.

use tork::api_model;

/// A user as returned by the API.
#[api_model]
pub struct UserOut {
    /// Unique identifier.
    pub id: i64,
    /// Email address.
    pub email: String,
    /// Display name.
    pub name: String,
}
