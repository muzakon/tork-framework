//! Confirms route-level hooks declared via the route macro attribute fire for
//! their route only.

use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use http_body_util::Full;
use tork::{App, AppInner, Method, ReqBody, RequestEvent, Router, StatusCode, box_body, get};

static AUDIT_HITS: AtomicUsize = AtomicUsize::new(0);

/// A named route-level hook (attributes cannot carry closures).
async fn audit(_event: RequestEvent) {
    AUDIT_HITS.fetch_add(1, Ordering::SeqCst);
}

#[get("/audited", on_request = audit)]
async fn audited() -> tork::Result<i64> {
    Ok(1)
}

#[get("/plain")]
async fn plain() -> tork::Result<i64> {
    Ok(2)
}

fn app() -> AppInner {
    App::new()
        .include_router(
            Router::new()
                .route(__tork_route_audited())
                .route(__tork_route_plain()),
        )
        .build()
        .unwrap()
}

fn request(uri: &str) -> http::Request<ReqBody> {
    http::Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(box_body(Full::new(Bytes::new())))
        .unwrap()
}

#[tokio::test]
async fn route_attribute_hook_fires_for_its_route_only() {
    let app = app();
    let before = AUDIT_HITS.load(Ordering::SeqCst);

    let response = app.dispatch(request("/audited")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(AUDIT_HITS.load(Ordering::SeqCst), before + 1);

    let _ = app.dispatch(request("/plain")).await;
    assert_eq!(
        AUDIT_HITS.load(Ordering::SeqCst),
        before + 1,
        "the plain route must not fire the audit hook"
    );
}
