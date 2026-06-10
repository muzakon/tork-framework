//! Order models.

use tork::api_model;

/// An order as returned by the API.
#[api_model(rename_all = "camelCase")]
pub struct OrderOut {
    /// Unique identifier.
    pub id: i64,
    /// Owning user's id.
    pub user_id: i64,
    /// Order total, in cents.
    pub total_cents: i64,
}
