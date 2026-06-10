//! Binary entrypoint for the example application.

use tork::middleware::{Compression, Cors, RequestId, Trace};
use tork::{
    App, AsyncApi, ErrorContext, HeaderName, HeaderValue, IntoResponse, Next, OpenApi, PanicEvent,
    Request, RequestEvent, Response, ResponseEvent, Result, WebSocketConfig, WsConnectInfo,
    WsDisconnectInfo, middleware,
};

use my_api::core::app_state::AppState;
use my_api::core::errors::RepoError;
use my_api::routers::{demo, health, users};

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

/// Maps a `RepoError` into a tailored response.
async fn handle_repo_error(error: RepoError, ctx: ErrorContext) -> Response {
    eprintln!("my_api: repo error on {}: {error}", ctx.path());
    tork::Error::service_unavailable("the data store is temporarily unavailable").into_response()
}

#[tork::main]
async fn main() -> tork::Result<()> {
    App::new()
        .lifespan::<AppState>()
        .catch_panics()
        .websocket_config(
            WebSocketConfig::new()
                .max_message_size_kb(64)
                .idle_timeout_secs(300),
        )
        .on_ws_connect(|info: WsConnectInfo| async move {
            println!("ws connect {}", info.path());
        })
        .on_ws_disconnect(|info: WsDisconnectInfo| async move {
            println!(
                "ws disconnect {} after {:.1?} ({:?})",
                info.path(),
                info.duration(),
                info.close_code()
            );
        })
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
        .include_router(health::router())
        .include_router(demo::router())
        .exception_handler::<RepoError, _, _>(handle_repo_error)
        .on_request(|event: RequestEvent| async move {
            println!("--> {} {}", event.method(), event.path());
        })
        .on_response(|event: ResponseEvent| async move {
            println!(
                "<-- {} {} {} ({:.1?})",
                event.method(),
                event.path(),
                event.status(),
                event.elapsed()
            );
        })
        .on_panic(|event: PanicEvent| async move {
            eprintln!("my_api: handler panicked on {}: {}", event.path(), event.message());
        })
        .openapi(
            OpenApi::new()
                .title("My API")
                .version("1.0.0")
                .json("/openapi.json")
                .docs("/docs"),
        )
        .asyncapi(
            AsyncApi::new()
                .title("My API (realtime)")
                .version("1.0.0")
                .json("/asyncapi.json"),
        )
        .on_ready(|ctx| async move {
            println!("my_api listening on {}", ctx.addr());
            Ok(())
        })
        .serve("0.0.0.0:8000")
        .await
}
