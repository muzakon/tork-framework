//! User routes.

use tork::{State, api_router, get};

use crate::core::app_state::AppState;
use crate::models::user::UserOut;

#[api_router(prefix = "/users", tags = ["users"])]
pub mod users_router {
    use super::*;

    /// Returns a single user by id.
    #[get("/{user_id}", response_model = UserOut, summary = "Get user by id")]
    pub async fn get_user(user_id: i64, state: State<AppState>) -> tork::Result<UserOut> {
        state.0.get_user(user_id)
    }
}

/// Builds the users router, ready to be mounted on the application.
pub fn router() -> tork::Router {
    users_router::router()
}
