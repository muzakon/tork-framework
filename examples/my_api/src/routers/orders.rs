//! Order routes, nested under a user.

use tork::{api_router, get, post, Valid};

use crate::core::auth::CurrentUser;
use crate::models::order::{CreateOrderInput, OrderOut};
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

    /// Creates an order for a user.
    #[post(
        "/",
        response_model = OrderOut,
        status_code = 201,
        summary = "Create an order for a user"
    )]
    pub async fn create_order(
        user_id: i64,
        current_user: CurrentUser,
        body: Valid<CreateOrderInput>,
        service: OrderService,
    ) -> tork::Result<OrderOut> {
        current_user.ensure_can_access_user(user_id)?;

        service.create_order(user_id, body.into_inner()).await
    }
}
