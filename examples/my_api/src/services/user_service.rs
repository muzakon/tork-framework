//! User service.

use tork::Inject;

use crate::models::user::UserOut;
use crate::repositories::user_repository::UserRepository;

/// Business logic for users.
///
/// Injects the [`UserRepository`], which in turn injects the store resource.
#[derive(Inject)]
pub struct UserService {
    users: UserRepository,
}

impl UserService {
    /// Returns a user by id.
    pub async fn get_user(&self, id: i64) -> tork::Result<UserOut> {
        self.users.find_by_id(id).await
    }
}
