//! User repository.

use tork::State;

use crate::core::app_state::AppState;
use crate::models::user::UserOut;

/// Data-access layer for users.
///
/// It is a Tork dependency: the generated `FromRequest` implementation resolves
/// the application state and constructs the repository on each request.
#[derive(Clone)]
pub struct UserRepository {
    state: AppState,
}

#[tork::dependency]
impl UserRepository {
    /// Builds the repository from the application state.
    pub async fn resolve(state: State<AppState>) -> tork::Result<Self> {
        Ok(Self { state: state.0 })
    }
}

impl UserRepository {
    /// Finds a user by id.
    pub async fn find_by_id(&self, id: i64) -> tork::Result<UserOut> {
        self.state.get_user(id)
    }

    /// Resolves a bearer token to the authenticated user's id.
    pub fn authenticate(&self, token: &str) -> tork::Result<i64> {
        self.state.authenticate(token)
    }
}
