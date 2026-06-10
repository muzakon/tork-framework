//! Order routes, nested under a user.

use tork::{api_router, get};

use crate::core::auth::CurrentUser;
use crate::models::order::OrderOut;
use crate::services::order_service::OrderService;

#[api_router(prefix = "/{user_id}/orders", tags = ["orders"])]
pub mod orders_router {
    use super::*;

    /// Lists the orders belonging to a user.
    #[get("/", response_model = Vec<OrderOut>, summary = "List orders for a user")]
    pub async fn list_user_orders(
        user_id: i64,
        current_user: CurrentUser,
        service: OrderService,
    ) -> tork::Result<Vec<OrderOut>> {
        current_user.ensure_can_access_user(user_id)?;

        service.list_orders_for_user(user_id).await
    }
}
