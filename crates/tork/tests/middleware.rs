//! Confirms the middleware surface is reachable through the facade crate, and
//! that the `#[middleware]` macro produces a working layer.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::HeaderValue;
use http_body_util::{BodyExt, Full, StreamBody};
use tork::testing::{LogRecorder, TestClient};
use tork::{
    box_body, bytes_response, get, middleware, App, BoxFuture, DuplicatePolicy, HandlerFn,
    LoggerConfig, Method, Middleware, Next, ReqBody, Request, RequestContext, Response, Result,
    Route, Router, StatusCode,
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
    std::sync::Arc::new(
        |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async {
                Ok(bytes_response(
                    StatusCode::OK,
                    "text/plain; charset=utf-8",
                    Bytes::from_static(b"ok"),
                ))
            })
        },
    )
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

fn app_with_post<M: Middleware>(mw: M) -> std::sync::Arc<tork::AppInner> {
    std::sync::Arc::new(
        App::new()
            .middleware(mw)
            .include_router(Router::new().route(tork::Route::new(Method::POST, "/", ok_handler())))
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
    Arc::new(
        |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(bytes_response(
                    StatusCode::OK,
                    "text/plain; charset=utf-8",
                    Bytes::from_static(b"slow"),
                ))
            })
        },
    )
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

    // An untrusted forwarded scheme is ignored.
    let passed = app_with(HttpsRedirect::new())
        .handle(get_with_headers(&[("x-forwarded-proto", "https")]))
        .await;
    assert_eq!(passed.status(), StatusCode::PERMANENT_REDIRECT);
}

#[tokio::test]
async fn proxy_headers_ignore_untrusted_forwarded_host() {
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn proxy_headers_rewrite_host_for_trusted_proxies() {
    use tork::middleware::{ProxyHeaders, TrustedHost};

    let client = TestClient::serve(
        App::new()
            .middleware(ProxyHeaders::new().trust_loopback())
            .middleware(TrustedHost::new(["real.example.com"]))
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler()))),
    )
    .bind_random_port()
    .await
    .unwrap();

    let response = client
        .get("/")
        .unsafe_header("host", "proxy.internal")
        .unsafe_header("x-forwarded-host", "real.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn cors_answers_preflight() {
    use tork::middleware::Cors;

    let app = app_with(
        Cors::new()
            .allow_origin("https://app.example.com")
            .allow_methods(["GET", "POST"])
            .allow_headers(["Authorization", "Content-Type"]),
    );
    let preflight = http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/")
        .header("origin", "https://app.example.com")
        .header("access-control-request-method", "POST")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app.handle(preflight).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "https://app.example.com"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-methods")
            .unwrap(),
        "GET, POST"
    );
}

#[tokio::test]
async fn cors_annotates_actual_request() {
    use tork::middleware::Cors;

    let app = app_with(
        Cors::new()
            .allow_origin("*")
            .expose_headers(["X-Request-Id"]),
    );
    let response = app
        .handle(get_with_headers(&[("origin", "https://anywhere.test")]))
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-expose-headers")
            .unwrap(),
        "X-Request-Id"
    );
}

fn large_handler() -> HandlerFn {
    Arc::new(
        |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async {
                Ok(bytes_response(
                    StatusCode::OK,
                    "text/plain; charset=utf-8",
                    Bytes::from(vec![b'a'; 2000]),
                ))
            })
        },
    )
}

fn app_with_handler<M: Middleware>(mw: M, handler: HandlerFn) -> Arc<tork::AppInner> {
    Arc::new(
        App::new()
            .middleware(mw)
            .include_router(Router::new().route(Route::new(Method::GET, "/", handler)))
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn compression_gzips_large_responses() {
    use tork::middleware::Compression;

    let app = app_with_handler(
        Compression::new().gzip().minimum_size(1000),
        large_handler(),
    );
    let response = app
        .handle(get_with_headers(&[("accept-encoding", "gzip")]))
        .await;

    assert_eq!(response.headers().get("content-encoding").unwrap(), "gzip");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let mut decoder = flate2::read::GzDecoder::new(&body[..]);
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
    assert_eq!(decoded, vec![b'a'; 2000]);
}

#[tokio::test]
async fn compression_skips_bodies_over_the_maximum_size() {
    use tork::middleware::Compression;

    // The 2000-byte body exceeds the 1500-byte cap, so it is sent uncompressed
    // (and, advertising a Content-Length, is never buffered for compression).
    let app = app_with_handler(
        Compression::new()
            .gzip()
            .minimum_size(1000)
            .maximum_size(1500),
        large_handler(),
    );
    let response = app
        .handle(get_with_headers(&[("accept-encoding", "gzip")]))
        .await;

    assert!(response.headers().get("content-encoding").is_none());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, Bytes::from(vec![b'a'; 2000]));
}

#[tokio::test]
async fn compression_skips_without_accept_encoding() {
    use tork::middleware::Compression;

    let app = app_with_handler(
        Compression::new().gzip().minimum_size(1000),
        large_handler(),
    );
    // `request()` carries no Accept-Encoding.
    let response = app.handle(request()).await;
    assert!(response.headers().get("content-encoding").is_none());
}

#[test]
fn duplicate_singleton_middleware_is_rejected_at_build() {
    use tork::middleware::Cors;

    let result = App::new()
        .middleware(Cors::new())
        .middleware(Cors::new())
        .build();

    let error = result.err().expect("duplicate Cors should be rejected");
    let message = error.message();
    assert!(
        message.contains("Duplicate middleware detected: Cors"),
        "message: {message}"
    );
    assert!(
        message.contains("can only be registered once per scope"),
        "message: {message}"
    );
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

#[tokio::test]
async fn request_id_supports_custom_header_name() {
    use tork::middleware::RequestId;
    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/")
        .header("x-correlation-id", "corr-1")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app_with(RequestId::new().header_name("x-correlation-id"))
        .handle(req)
        .await;
    assert_eq!(
        response.headers().get("x-correlation-id").unwrap(),
        "corr-1"
    );
}

#[tokio::test]
async fn trace_middleware_logs_success_and_errors() {
    use tork::middleware::Trace;

    let recorder = LogRecorder::new();
    let client = TestClient::builder(
        App::new()
            .logger(LoggerConfig::new().request_logs(false))
            .middleware(Trace::new())
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler()))),
    )
    .logger(recorder.clone())
    .build()
    .await
    .unwrap();

    assert_eq!(client.get("/").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/missing").send().await.unwrap().status(), 404);

    let records = recorder.records();
    assert!(records.iter().any(|r| r.message.contains("GET / 200")));
    assert!(records
        .iter()
        .any(|r| r.message.contains("GET /missing 404")));
}

#[tokio::test]
async fn trusted_host_accepts_host_with_port() {
    use tork::middleware::TrustedHost;

    let response = app_with(TrustedHost::new(["example.com"]))
        .handle(get_with_headers(&[("host", "example.com:8443")]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn cors_wildcard_with_credentials_fails_closed_and_keeps_other_headers() {
    use tork::middleware::Cors;

    let app = app_with(
        Cors::new()
            .allow_origin("*")
            .allow_credentials(true)
            .allow_headers(["Authorization"])
            .allow_methods(["GET"])
            .max_age(600),
    );
    let preflight = http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/")
        .header("origin", "https://app.example.com")
        .header("access-control-request-method", "GET")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app.handle(preflight).await;
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-credentials")
            .unwrap(),
        "true"
    );
    assert_eq!(
        response.headers().get("access-control-max-age").unwrap(),
        "600"
    );
}

#[tokio::test]
async fn https_redirect_preserves_path_and_query() {
    use tork::middleware::HttpsRedirect;

    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/search?q=tork")
        .header("host", "example.com")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app_with(HttpsRedirect::new()).handle(req).await;
    assert_eq!(
        response.headers().get("location").unwrap(),
        "https://example.com/search?q=tork"
    );
}

#[tokio::test]
async fn proxy_headers_spoofing_bypasses_trusted_host_without_proxy_headers() {
    use tork::middleware::TrustedHost;

    let app = Arc::new(
        App::new()
            .middleware(TrustedHost::new(["real.example.com"]))
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler())))
            .build()
            .unwrap(),
    );

    let response = app
        .handle(get_with_headers(&[
            ("host", "evil.com"),
            ("x-forwarded-host", "real.example.com"),
        ]))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn https_redirect_spoofing_via_x_forwarded_proto() {
    use tork::middleware::HttpsRedirect;

    let app = app_with(HttpsRedirect::new());

    let response = app
        .clone()
        .handle(get_with_headers(&[
            ("host", "example.com"),
            ("x-forwarded-proto", "https"),
        ]))
        .await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);

    let response = app
        .handle(get_with_headers(&[
            ("host", "example.com"),
            ("x-forwarded-proto", "http"),
        ]))
        .await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
}

#[tokio::test]
async fn https_redirect_honors_trusted_proxy_scheme() {
    use tork::middleware::{HttpsRedirect, ProxyHeaders};

    let client = TestClient::serve(
        App::new()
            .middleware(ProxyHeaders::new().trust_loopback())
            .middleware(HttpsRedirect::new())
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler()))),
    )
    .bind_random_port()
    .await
    .unwrap();

    let response = client
        .get("/")
        .unsafe_header("host", "example.com")
        .unsafe_header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn cors_wildcard_with_credentials_is_rejected() {
    use tork::middleware::Cors;

    let app = app_with(Cors::new().allow_origin("*").allow_credentials(true));

    let response = app
        .handle(get_with_headers(&[("origin", "https://evil.example.com")]))
        .await;
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn cors_preflight_includes_full_vary_headers() {
    use tork::middleware::Cors;

    let app = app_with(
        Cors::new()
            .allow_origin("https://app.example.com")
            .allow_methods(["GET", "POST"])
            .allow_headers(["Authorization", "Content-Type"]),
    );

    let preflight = http::Request::builder()
        .method(Method::OPTIONS)
        .uri("/")
        .header("origin", "https://app.example.com")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "Authorization")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();

    let response = app.handle(preflight).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let vary = response.headers().get("vary").unwrap().to_str().unwrap();
    assert!(vary.contains("Origin"));
    assert!(vary.contains("Access-Control-Request-Method"));
    assert!(vary.contains("Access-Control-Request-Headers"));
}

#[tokio::test]
async fn cors_actual_request_includes_vary_origin() {
    use tork::middleware::Cors;

    let app = app_with(Cors::new().allow_origin("https://app.example.com"));
    let response = app
        .handle(get_with_headers(&[("origin", "https://app.example.com")]))
        .await;

    let vary = response.headers().get("vary").unwrap().to_str().unwrap();
    assert!(vary.contains("Origin"));
}

#[tokio::test]
async fn trusted_host_accepts_bracketed_ipv6_with_a_port() {
    use tork::middleware::TrustedHost;

    // A bracketed IPv6 host with a port is parsed correctly and matches the
    // allowlisted literal, instead of being mangled and rejected.
    let allowed = app_with(TrustedHost::new(["[::1]", "localhost"]))
        .handle(get_with_headers(&[("host", "[::1]:8080")]))
        .await;
    assert_eq!(allowed.status(), StatusCode::OK);

    // An IPv6 host not on the allowlist is still rejected.
    let rejected = app_with(TrustedHost::new(["[::1]"]))
        .handle(get_with_headers(&[("host", "[2001:db8::1]:443")]))
        .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn panic_recovery_returns_500_by_default() {
    fn panic_handler() -> HandlerFn {
        Arc::new(
            |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async {
                    panic!("intentional panic for testing");
                })
            },
        )
    }

    let app = Arc::new(
        App::new()
            .include_router(Router::new().route(Route::new(Method::GET, "/panic", panic_handler())))
            .build()
            .unwrap(),
    );

    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/panic")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();
    let response = app.handle(req).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("Internal server error"));
}

#[tokio::test]
async fn panic_recovery_with_test_client_returns_500() {
    #[get("/panic")]
    async fn panic_endpoint() -> tork::Result<serde_json::Value> {
        panic!("intentional panic for testing");
    }

    let app = App::new()
        .include(panic_endpoint)
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let response = client.get("/panic").send().await.unwrap();
    assert_eq!(response.status(), 500);
}

#[tokio::test]
async fn body_limit_chunked_requests_are_rejected_once_the_stream_crosses_the_limit() {
    use bytes::Bytes;
    use http_body::Frame;
    use tork::middleware::BodyLimit;

    let app = app_with_post(BodyLimit::bytes(100));

    let chunks: Vec<Result<Frame<Bytes>, std::convert::Infallible>> = (0..50)
        .map(|_| Ok(Frame::data(Bytes::from_static(b"xxxxxxxxxx"))))
        .collect();
    let body = StreamBody::new(stream::iter(chunks));

    let req = http::Request::builder()
        .method(Method::POST)
        .uri("/")
        .header("transfer-encoding", "chunked")
        .body(box_body(body))
        .unwrap();

    let response = app.handle(req).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn body_limit_allows_chunked_requests_under_limit() {
    use bytes::Bytes;
    use http_body::Frame;
    use tork::middleware::BodyLimit;

    let app = app_with_post(BodyLimit::bytes(1000));

    let chunks: Vec<Result<Frame<Bytes>, std::convert::Infallible>> = (0..5)
        .map(|_| Ok(Frame::data(Bytes::from_static(b"xxxxxxxxxx"))))
        .collect();
    let body = StreamBody::new(stream::iter(chunks));

    let req = http::Request::builder()
        .method(Method::POST)
        .uri("/")
        .header("transfer-encoding", "chunked")
        .body(box_body(body))
        .unwrap();

    let response = app.handle(req).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn security_headers_set_baseline_on_responses() {
    use tork::middleware::SecurityHeaders;

    let response = app_with(SecurityHeaders::new()).handle(request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers.get("strict-transport-security").unwrap(),
        "max-age=31536000; includeSubDomains"
    );
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");
    // CSP is off by default.
    assert!(headers.get("content-security-policy").is_none());
}

#[tokio::test]
async fn security_headers_respect_builder_and_handler_overrides() {
    use tork::middleware::SecurityHeaders;

    let mw = SecurityHeaders::new()
        .frame_options("SAMEORIGIN")
        .content_security_policy("default-src 'self'")
        .without_hsts();
    let response = app_with(mw).handle(request()).await;
    let headers = response.headers();
    assert_eq!(headers.get("x-frame-options").unwrap(), "SAMEORIGIN");
    assert_eq!(
        headers.get("content-security-policy").unwrap(),
        "default-src 'self'"
    );
    // without_hsts() drops the HSTS header entirely.
    assert!(headers.get("strict-transport-security").is_none());
}

#[tokio::test]
async fn security_headers_rejects_duplicate_registration() {
    use tork::middleware::SecurityHeaders;

    let err = App::new()
        .middleware(SecurityHeaders::new())
        .middleware(SecurityHeaders::new())
        .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler())))
        .build()
        .err()
        .expect("duplicate SecurityHeaders should be rejected");
    assert!(err.to_string().contains("SecurityHeaders"));
}

#[tokio::test]
async fn compression_sets_vary_even_when_not_compressing() {
    use tork::middleware::Compression;

    // gzip is enabled but the request does not accept it, so the response is not
    // compressed; it must still advertise `Vary: Accept-Encoding` so caches do not
    // hand it to a gzip-expecting client.
    let response = app_with(Compression::new().gzip()).handle(request()).await;

    assert!(response.headers().get("content-encoding").is_none());
    let varies = response.headers().get_all("vary").iter().any(|value| {
        value
            .to_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("accept-encoding")
    });
    assert!(varies, "expected Vary: Accept-Encoding");
}

#[tokio::test]
async fn error_responses_are_not_cacheable() {
    let app = std::sync::Arc::new(
        App::new()
            .include_router(Router::new().route(Route::new(Method::GET, "/", ok_handler())))
            .build()
            .unwrap(),
    );

    // A request to a missing route yields a 404 error response.
    let req = http::Request::builder()
        .method(Method::GET)
        .uri("/missing")
        .body(box_body(Full::new(Bytes::new())))
        .unwrap();
    let response = app.handle(req).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
}
