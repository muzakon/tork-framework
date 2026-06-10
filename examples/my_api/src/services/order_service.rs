//! Order service.

use tork::Inject;

use crate::core::app_state::UserStore;
use crate::models::order::{CreateOrderInput, OrderOut};

/// Business logic for orders.
#[derive(Inject)]
pub struct OrderService {
    store: UserStore,
}

impl OrderService {
    /// Lists the orders belonging to a user.
    pub async fn list_orders_for_user(&self, user_id: i64) -> tork::Result<Vec<OrderOut>> {
        Ok(self.store.orders_for(user_id))
    }

    /// Creates an order for a user.
    ///
    /// The example does not persist; it echoes back a created order with a
    /// computed total. The input has already been validated by the extractor.
    pub async fn create_order(
        &self,
        user_id: i64,
        input: CreateOrderInput,
    ) -> tork::Result<OrderOut> {
        let total_cents = ((input.price + input.tax.unwrap_or(0.0)) * 100.0).round() as i64;
        Ok(OrderOut {
            id: 9999,
            user_id,
            total_cents,
        })
    }
}
