//! Order models.

use tork::api_model;

/// Custom validator: rejects values that are empty once trimmed.
///
/// Demonstrates a Pydantic-style custom validator with its own message. garde
/// passes the field by reference; `&String` coerces to `&str` here.
fn not_blank(value: &str, _ctx: &()) -> garde::Result {
    if value.trim().is_empty() {
        Err(garde::Error::new("must not be blank"))
    } else {
        Ok(())
    }
}

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
    #[field(min_length = 1, max_length = 120, custom = not_blank)]
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
