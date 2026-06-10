//! User routes.

use tork::{api_router, get};

use crate::models::user::UserOut;
use crate::services::user_service::UserService;

#[api_router(prefix = "/users", tags = ["users"])]
pub mod users_router {
    use super::*;

    /// Returns a single user by id.
    #[get("/{user_id}", response_model = UserOut, summary = "Get user by id")]
    pub async fn get_user(user_id: i64, service: UserService) -> tork::Result<UserOut> {
        service.get_user(user_id).await
    }
}

/// Builds the users router, including the nested orders router.
pub fn router() -> tork::Router {
    users_router::router().include(crate::routers::orders::orders_router::router())
}
