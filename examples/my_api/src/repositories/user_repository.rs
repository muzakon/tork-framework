//! User repository.

use tork::Inject;

use crate::core::app_state::UserStore;
use crate::models::user::UserOut;

/// Data-access layer for users.
///
/// A Tork `Inject` service: the store resource is injected by type on each
/// request, so the repository never reaches through a state object.
#[derive(Inject)]
pub struct UserRepository {
    store: UserStore,
}

impl UserRepository {
    /// Finds a user by id.
    pub async fn find_by_id(&self, id: i64) -> tork::Result<UserOut> {
        self.store.get_user(id)
    }

    /// Resolves a bearer token to the authenticated user's id.
    pub fn authenticate(&self, token: &str) -> tork::Result<i64> {
        self.store.authenticate(token)
    }
}
