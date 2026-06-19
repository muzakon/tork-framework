//! The authenticated-user guard and authorization helpers.

use std::sync::Arc;

use tork::{BearerToken, Error};

use crate::config::AuthConfig;
use crate::security::decode_token;
use crate::users::UserRepository;

/// The authenticated user for the current request.
///
/// This is a `#[tork::dependency]`: a handler that takes a `CurrentUser`
/// parameter triggers `resolve`, which itself depends on the `BearerToken`
/// extractor, the `UserRepository`, and the `AuthConfig`. Tork resolves those
/// first, then runs `resolve`. If any step fails, the request is rejected before
/// the handler runs and no `CurrentUser` is ever built.
#[derive(Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub scopes: Vec<String>,
}

#[tork::dependency]
impl CurrentUser {
    pub async fn resolve(
        token: BearerToken,
        repo: UserRepository,
        config: Arc<AuthConfig>,
    ) -> tork::Result<Self> {
        // Verify the JWT signature and expiry, then load the user it names.
        let claims = decode_token(&config, token.token())?;
        let user_id: i64 = claims
            .sub
            .parse()
            .map_err(|_| Error::unauthorized("malformed token subject"))?;
        let user = repo.find_by_id(user_id)?;

        // A valid token for a disabled account is authenticated but not allowed.
        if user.disabled {
            return Err(Error::forbidden("this account is disabled"));
        }

        Ok(CurrentUser {
            id: user.id,
            username: user.username,
            // Trust the scopes signed into the token, not the live user record,
            // so a token can be issued with a subset of a user's scopes.
            role: user.role,
            scopes: claims.scopes,
        })
    }
}

impl CurrentUser {
    /// Ensures the current user is acting on their own account.
    pub fn ensure_can_access_user(&self, user_id: i64) -> tork::Result<()> {
        if self.id != user_id {
            return Err(Error::forbidden("you can only access your own account"));
        }
        Ok(())
    }

    /// Ensures the current user's token carries `scope`.
    pub fn require_scope(&self, scope: &str) -> tork::Result<()> {
        if !self.scopes.iter().any(|granted| granted == scope) {
            return Err(Error::forbidden(format!("missing required scope `{scope}`")));
        }
        Ok(())
    }

    /// Ensures the current user has `role`.
    pub fn require_role(&self, role: &str) -> tork::Result<()> {
        if self.role != role {
            return Err(Error::forbidden(format!("requires the `{role}` role")));
        }
        Ok(())
    }
}
