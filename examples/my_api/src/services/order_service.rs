//! Order service.

use tork::State;

use crate::core::app_state::AppState;
use crate::models::order::OrderOut;

/// Business logic for orders.
pub struct OrderService {
    state: AppState,
}

#[tork::dependency]
impl OrderService {
    /// Builds the service from the application state.
    pub async fn resolve(state: State<AppState>) -> tork::Result<Self> {
        Ok(Self { state: state.0 })
    }
}

impl OrderService {
    /// Lists the orders belonging to a user.
    pub async fn list_orders_for_user(&self, user_id: i64) -> tork::Result<Vec<OrderOut>> {
        Ok(self.state.orders_for(user_id))
    }
}
