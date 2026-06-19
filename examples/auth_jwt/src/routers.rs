//! The authentication and authorization endpoints.

use std::sync::Arc;

use tork::{api_router, get, post, Error, Valid};

use crate::auth::CurrentUser;
use crate::config::AuthConfig;
use crate::schemas::{LoginInput, TokenResponse, UserOut};
use crate::security::{encode_token, verify_password};
use crate::users::UserRepository;

#[api_router(tags = ["auth"])]
pub mod auth_router {
    use super::*;

    /// Logs in with a username and password and returns a signed JWT. This is
    /// the only public endpoint; the others below require the token.
    #[post("/token", response_model = TokenResponse, summary = "Log in and get a token")]
    pub async fn login(
        repo: UserRepository,
        config: Arc<AuthConfig>,
        body: Valid<LoginInput>,
    ) -> tork::Result<TokenResponse> {
        let input = body.into_inner();

        let user = repo
            .find_by_username(&input.username)
            .filter(|user| verify_password(&input.password, &user.password_hash))
            .ok_or_else(|| Error::unauthorized("incorrect username or password"))?;

        // Sign the user's scopes into the token so authorization checks can read
        // them back without another lookup.
        let token = encode_token(&config, user.id, user.scopes.clone())?;
        Ok(TokenResponse {
            access_token: token,
            token_type: "bearer".to_owned(),
        })
    }

    /// Returns the current user. Requires a valid bearer token, declared with
    /// `security` so the OpenAPI docs show the Authorize button on it.
    #[get("/users/me", response_model = UserOut, security = ["bearerAuth"], summary = "Current user")]
    pub async fn read_current_user(current_user: CurrentUser) -> tork::Result<UserOut> {
        Ok(UserOut {
            id: current_user.id,
            username: current_user.username,
            role: current_user.role,
        })
    }

    /// An endpoint that needs a specific scope. The token must carry
    /// `users:write`; the requirement is also declared to the docs.
    #[get(
        "/admin/overview",
        response_model = UserOut,
        security = [bearerAuth(scopes = ["users:write"])],
        summary = "Admin overview"
    )]
    pub async fn admin_overview(current_user: CurrentUser) -> tork::Result<UserOut> {
        current_user.require_scope("users:write")?;
        Ok(UserOut {
            id: current_user.id,
            username: current_user.username,
            role: current_user.role,
        })
    }
}

pub use auth_router::router;
