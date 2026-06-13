//! Confirms `#[derive(AppError)]` converts a user error through `?` and that a
//! registered `exception_handler` recovers and maps the typed value.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use tork::{
    box_body, App, AppInner, BoxFuture, HandlerFn, IntoResponse, Method, ReqBody, RequestContext,
    Response, Result, Route, Router, StatusCode,
};

/// A user error that converts into `tork::Error` and defaults to `503`.
#[derive(Debug, PartialEq, tork::AppError)]
#[status(503)]
enum DbError {
    Timeout,
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("database timed out")
    }
}

impl std::error::Error for DbError {}

/// A route whose handler surfaces a `DbError` through `?`.
fn failing_route() -> Router {
    let handler: HandlerFn = Arc::new(
        |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async {
                // The `?` converts `DbError` into `tork::Error` via the derived `From`.
                let outcome: std::result::Result<(), DbError> = Err(DbError::Timeout);
                outcome?;
                Ok(StatusCode::OK.into_response())
            })
        },
    );
    Router::new().route(Route::new(Method::GET, "/db", handler))
}

fn request() -> http::Request<ReqBody> {
    http::Request::builder()
        .method(Method::GET)
        .uri("/db")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap()
}

#[tokio::test]
async fn derived_error_uses_its_declared_status_by_default() {
    let app: AppInner = App::new().include_router(failing_route()).build().unwrap();

    let response = app.dispatch(request()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn exception_handler_recovers_and_maps_the_typed_error() {
    let app: AppInner = App::new()
        .include_router(failing_route())
        .exception_handler::<DbError, _, _>(|error, _ctx| async move {
            // The original typed value is recovered.
            assert_eq!(error, DbError::Timeout);
            StatusCode::IM_A_TEAPOT.into_response()
        })
        .build()
        .unwrap();

    let response = app.dispatch(request()).await;
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
}
