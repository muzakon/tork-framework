//! Binary entrypoint for the example application.

use tork::{App, OpenApi};

use my_api::core::app_state::AppState;
use my_api::routers::users;

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .include_router(users::router())
        .openapi(
            OpenApi::new()
                .title("My API")
                .version("1.0.0")
                .json("/openapi.json")
                .docs("/docs"),
        )
        .serve("0.0.0.0:8000")
        .await
}
