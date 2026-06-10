//! Binary entrypoint for the example application.

use tork::App;

use my_api::core::app_state::AppState;
use my_api::routers::users;

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .include_router(users::router())
        .serve("0.0.0.0:8000")
        .await
}
