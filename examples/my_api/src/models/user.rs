//! User-facing models.

use serde::Serialize;

/// A user as returned by the API.
#[derive(Clone, Serialize)]
pub struct UserOut {
    /// Unique identifier.
    pub id: i64,
    /// Email address.
    pub email: String,
    /// Display name.
    pub name: String,
}
