//! Confirms the middleware surface is reachable through the facade crate, and
//! that the `#[middleware]` macro produces a working layer.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::HeaderValue;
use http_body_util::{BodyExt, Full};
use tork::{
    App, BoxFuture, DuplicatePolicy, HandlerFn, Method, Middleware, Next, ReqBody, Request,
    RequestContext, Response, Result, Route, Router, StatusCode, box_body, bytes_response,
    middleware,
};

struct Noop;

impl Middleware for Noop {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        next.run(request)
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Allow
    }
}

#[test]
fn middleware_types_are_exported() {
    assert!(!Noop.name().is_empty());
    assert_eq!(Noop.duplicate_policy(), DuplicatePolicy::Allow);
}

#[middleware]
async fn add_marker(req: Request, next: Next) -> Result<Response> {
    let mut res = next.run(req).await?;
    res.headers_mut()
        .insert("x-marker", HeaderValue::from_static("on"));
    Ok(res)
}

fn ok_handler() -> HandlerFn {
    std::sync::Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Response> {
        Box::pin(async {
            bytes_response(StatusCode::OK, "text/plain; charset=utf-8", Bytes::from_static(b"ok"))
        })
    })
}

fn request() -> http::Request<ReqBody> {
    http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap()
}

#[tokio::test]
async fn custom_middleware_macro_runs() {
    let app = std::sync::Arc::new(
        App::new()
            .middleware(add_marker)
            .include_router(Router::new().route(tork::Route::new(Method::GET, "/", ok_handler())))
            .build()
            .unwrap(),
    );

    let response = app.handle(request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-marker").unwrap(),
        HeaderValue::from_static("on")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

fn app_with<M: Middleware>(mw: M) -> std::sync::Arc<tork::AppInner> {
    std::sync::Arc::new(
        App::new()
            .middleware(mw)
            .include_router(Router::new().route(tork::Route::new(Method::GET, "/", ok_handler())))
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn request_id_generates_when_absent() {
    use tork::middleware::RequestId;
    let response = app_with(RequestId::new()).handle(request()).await;
    let id = response
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(id.starts_with("req-"), "id: {id}");
}

fn slow_handler() -> HandlerFn {
    Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Response> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            bytes_response(StatusCode::OK, "text/plain; charset=utf-8", Bytes::from_static(b"slow"))
        })
    })
}

#[tokio::test]
async fn timeout_returns_504_for_slow_handler() {
    use tork::middleware::Timeout;
    let app = Arc::new(
        App::new()
            .middleware(Timeout::millis(10))
            .include_router(Router::new().route(Route::new(Method::GET, "/", slow_handler())))
            .build()
            .unwrap(),
    );
    let response = app.handle(request()).await;
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn body_limit_rejects_oversized_content_length() {
    use tork::middleware::BodyLimit;
    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("content-length", "1000000")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();
    let response = app_with(BodyLimit::bytes(10)).handle(req).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

fn get_with_headers(headers: &[(&str, &str)]) -> http::Request<ReqBody> {
    let mut builder = http::Request::builder().method(Method::GET).uri("/");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(box_body(Full::new(Bytes::new()))).unwrap()
}

#[tokio::test]
async fn trusted_host_allows_and_rejects() {
    use tork::middleware::TrustedHost;

    let allowed = app_with(TrustedHost::new(["example.com", "*.example.com"]))
        .handle(get_with_headers(&[("host", "app.example.com")]))
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);

    let rejected = app_with(TrustedHost::new(["example.com"]))
        .handle(get_with_headers(&[("host", "evil.com")]))
        .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn https_redirect_redirects_plain_http() {
    use tork::middleware::HttpsRedirect;

    let response = app_with(HttpsRedirect::new())
        .handle(get_with_headers(&[("host", "example.com")]))
        .await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "https://example.com/"
    );

    // Already HTTPS (per the proxy header) passes through.
    let passed = app_with(HttpsRedirect::new())
        .handle(get_with_headers(&[("x-forwarded-proto", "https")]))
        .await;
    assert_eq!(passed.status(), StatusCode::OK);
}

#[tokio::test]
async fn proxy_headers_rewrites_host_for_trusted_host() {
    use tork::middleware::{ProxyHeaders, TrustedHost};

    let app = Arc::new(
        App::new()
            .middleware(ProxyHeaders::new())
            .middleware(TrustedHost::new(["real.example.com"]))
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler())))
            .build()
            .unwrap(),
    );

    // The direct Host is untrusted, but X-Forwarded-Host carries the real one.
    let response = app
        .handle(get_with_headers(&[
            ("host", "proxy.internal"),
            ("x-forwarded-host", "real.example.com"),
        ]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn request_id_propagates_incoming() {
    use tork::middleware::RequestId;
    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("x-request-id", "req-supplied")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();
    let response = app_with(RequestId::new()).handle(req).await;
    assert_eq!(
        response.headers().get("x-request-id").unwrap(),
        "req-supplied"
    );
}
