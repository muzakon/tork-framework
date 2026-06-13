//! Binary entrypoint for the example application.

use std::sync::Arc;

use tork::middleware::{Compression, Cors, RequestId};
use std::time::Duration;

use tork::{
    middleware, App, AsyncApi, ErrorContext, HeaderName, HeaderValue, Http1Config, Http2Config,
    IntoResponse, Logger, Next, OpenApi, PanicEvent, Request, Response, Result, WebSocketConfig,
    WsConnectInfo, WsDisconnectInfo,
};

use my_api::core::app_state::{AppState, Config};
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
    Logger::new("ExceptionHandler")
        .error("repository error")
        .field("path", ctx.path().to_owned())
        .error(&error)
        .emit();
    tork::Error::service_unavailable("the data store is temporarily unavailable").into_response()
}

#[tork::main]
async fn main() -> tork::Result<()> {
    // Load configuration first so it can configure logging, then register it as a
    // resource for handlers and services.
    let config = Config::load()?;
    let logger = config.logging.to_logger();
    let config = Arc::new(config);

    App::new()
        .logger(logger)
        .state(config)
        .lifespan::<AppState>()
        .catch_panics()
        // Server / protocol tuning. The request-head read timeout (slowloris guard)
        // is on by default; these add an 8 MiB body cap and HTTP/2 + HTTP/1 limits.
        .max_request_body_size(8 * 1024 * 1024)
        .header_read_timeout(Duration::from_secs(30))
        .http2(
            Http2Config::new()
                .max_concurrent_streams(256)
                .keep_alive_interval(Duration::from_secs(20))
                .keep_alive_timeout(Duration::from_secs(10)),
        )
        .http1(Http1Config::new().keep_alive(true))
        // Terminate TLS here by building with `--features tls` and adding:
        //     .tls(tork::TlsConfig::from_pem_files("cert.pem", "key.pem")?)
        // HTTP/2 is then negotiated automatically over ALPN. To serve over a Unix
        // socket instead of TCP, call `app.serve_unix("/tmp/my_api.sock").await`.
        .websocket_config(
            WebSocketConfig::new()
                .max_message_size_kb(64)
                .idle_timeout_secs(300),
        )
        .on_ws_connect(|info: WsConnectInfo| async move {
            Logger::new("WebSocket")
                .info("connected")
                .field("path", info.path().to_owned())
                .emit();
        })
        .on_ws_disconnect(|info: WsDisconnectInfo| async move {
            Logger::new("WebSocket")
                .info("disconnected")
                .field("path", info.path().to_owned())
                .field("duration_ms", info.duration().as_millis() as u64)
                .emit();
        })
        .middleware(RequestId::new())
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
        .on_panic(|event: PanicEvent| async move {
            Logger::new("ExceptionHandler")
                .error("handler panicked")
                .field("path", event.path().to_owned())
                .field("message", event.message().to_owned())
                .emit();
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
        .serve("0.0.0.0:8000")
        .await
}
