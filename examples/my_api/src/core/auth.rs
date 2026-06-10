//! Authentication: the current user dependency.

use tork::{BearerToken, Error};

use crate::repositories::user_repository::UserRepository;

/// The authenticated user for the current request.
///
/// This dependency depends on two other dependencies, `BearerToken` and
/// `UserRepository`, demonstrating recursive resolution: Tork resolves the
/// bearer token from the request headers and the repository from application
/// state, then runs `resolve`.
#[derive(Clone)]
pub struct CurrentUser {
    /// The authenticated user's id.
    pub id: i64,
    /// The authenticated user's email.
    pub email: String,
}

#[tork::dependency]
impl CurrentUser {
    /// Resolves the current user from the bearer token.
    pub async fn resolve(token: BearerToken, users: UserRepository) -> tork::Result<Self> {
        // A real service would verify a signed token here; the example maps the
        // opaque token to a user id through the repository.
        let user_id = users.authenticate(token.token())?;
        let user = users.find_by_id(user_id).await?;

        Ok(CurrentUser {
            id: user.id,
            email: user.email,
        })
    }
}

impl CurrentUser {
    /// Ensures the current user may act on behalf of `user_id`.
    ///
    /// # Errors
    ///
    /// Returns `403 Forbidden` if the ids differ.
    pub fn ensure_can_access_user(&self, user_id: i64) -> tork::Result<()> {
        if self.id != user_id {
            return Err(Error::forbidden("access denied"));
        }
        Ok(())
    }
}
