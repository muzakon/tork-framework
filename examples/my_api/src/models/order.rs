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

/// Payload for creating an order.
#[api_model(rename_all = "camelCase")]
pub struct CreateOrderInput {
    /// Order name.
    #[field(min_length = 1, max_length = 120)]
    pub name: String,

    /// Optional description.
    #[field(max_length = 300, title = "The description of the item")]
    pub description: Option<String>,

    /// Unit price; must be positive.
    #[field(gt = 0, description = "The price must be greater than zero")]
    pub price: f64,

    /// Optional tax; must not be negative.
    #[field(ge = 0)]
    pub tax: Option<f64>,
}
