//! Order models.

use serde::Serialize;

/// An order as returned by the API.
#[derive(Clone, Serialize)]
pub struct OrderOut {
    /// Unique identifier.
    pub id: i64,
    /// Owning user's id.
    pub user_id: i64,
    /// Order total, in cents.
    pub total_cents: i64,
}
