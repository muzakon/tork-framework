//! User service.

use crate::models::user::UserOut;
use crate::repositories::user_repository::UserRepository;

/// Business logic for users.
///
/// This dependency takes the [`UserRepository`], which is itself a dependency.
/// Tork resolves the chain automatically: building a `UserService` first builds
/// a `UserRepository`, which in turn reads the application state.
pub struct UserService {
    users: UserRepository,
}

#[tork::dependency]
impl UserService {
    /// Builds the service from the user repository.
    pub async fn resolve(users: UserRepository) -> tork::Result<Self> {
        Ok(Self { users })
    }
}

impl UserService {
    /// Returns a user by id.
    pub async fn get_user(&self, id: i64) -> tork::Result<UserOut> {
        self.users.find_by_id(id).await
    }
}
