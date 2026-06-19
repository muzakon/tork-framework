//! Binary entrypoint for the JWT auth example.

use std::sync::Arc;

use tork::{App, OpenApi};

use auth_jwt::config::AuthConfig;
use auth_jwt::routers;
use auth_jwt::users::UserStore;

#[tork::main]
async fn main() -> tork::Result<()> {
    let config = AuthConfig::load()?;

    App::new()
        // Register the secret config and the user store as shared resources;
        // handlers and guards receive them as `Arc<AuthConfig>` / `Arc<UserStore>`.
        .state(Arc::new(config))
        .state(Arc::new(UserStore::seed()))
        .include_router(routers::router())
        .openapi(
            OpenApi::new()
                .title("Auth Example")
                .version("1.0.0")
                // Register the bearer scheme so the docs UI shows an Authorize
                // button. Routes opt in with `security = ["bearerAuth"]`.
                .bearer_auth()
                .json("/openapi.json")
                .docs("/docs"),
        )
        .serve("0.0.0.0:8000")
        .await
}
