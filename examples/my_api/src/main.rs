//! Binary entrypoint for the example application.

use tork::middleware::{Compression, Cors, RequestId, Trace};
use tork::{App, HeaderName, HeaderValue, Next, OpenApi, Request, Response, Result, middleware};

use my_api::core::app_state::AppState;
use my_api::routers::users;

/// Custom middleware: records how long the request took to process.
#[middleware]
async fn add_process_time(req: Request, next: Next) -> Result<Response> {
    let start = std::time::Instant::now();
    let mut response = next.run(req).await?;

    let elapsed = start.elapsed().as_secs_f64();
    if let Ok(value) = HeaderValue::from_str(&format!("{elapsed:.6}")) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-process-time"), value);
    }
    Ok(response)
}

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .middleware(RequestId::new())
        .middleware(Trace::new())
        .middleware(add_process_time)
        .middleware(
            Cors::new()
                .allow_origin("*")
                .allow_methods(["GET", "POST"])
                .expose_headers(["X-Request-Id"]),
        )
        .middleware(Compression::new().gzip().minimum_size(256))
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
