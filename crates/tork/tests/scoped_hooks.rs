//! Confirms route-level hooks declared via the route macro attribute fire for
//! their route only.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::Full;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
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

// --- End-to-end: global + router-scoped + route-attribute hooks compose ---

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(label: impl Into<String>) {
    LOG.lock().unwrap().push(label.into());
}

fn log_count(label: &str) -> usize {
    LOG.lock().unwrap().iter().filter(|entry| *entry == label).count()
}

fn log_has(label: &str) -> bool {
    LOG.lock().unwrap().iter().any(|entry| entry == label)
}

/// A named route-level hook for the end-to-end test.
async fn e2e_route_hook(_event: RequestEvent) {
    record("route");
}

#[get("/admin/a")]
async fn admin_a() -> tork::Result<i64> {
    Ok(1)
}

#[get("/admin/b")]
async fn admin_b() -> tork::Result<i64> {
    Ok(1)
}

#[get("/tagged", on_request = e2e_route_hook)]
async fn tagged() -> tork::Result<i64> {
    Ok(1)
}

#[get("/plain2")]
async fn plain2() -> tork::Result<i64> {
    Ok(1)
}

async fn get_request(addr: std::net::SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn global_router_and_route_hooks_compose_over_tcp() {
    LOG.lock().unwrap().clear();

    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let admin = Router::new()
        .route(__tork_route_admin_a())
        .route(__tork_route_admin_b())
        .on_request(|_event: RequestEvent| async { record("router") });

    let other = Router::new()
        .route(__tork_route_tagged())
        .route(__tork_route_plain2());

    let app = App::new()
        .on_request(|event: RequestEvent| {
            let path = event.path().to_owned();
            async move { record(format!("global:{path}")) }
        })
        .include_router(admin)
        .include_router(other)
        .on_ready(move |ctx| {
            let sender = sender.clone();
            async move {
                if let Some(tx) = sender.lock().unwrap().take() {
                    let _ = tx.send(ctx.addr());
                }
                Ok(())
            }
        });

    let server = tokio::spawn(app.serve_with_shutdown("127.0.0.1:0", async move {
        let _ = shutdown_rx.await;
    }));

    let addr = addr_rx.await.unwrap();
    for path in ["/admin/a", "/admin/b", "/tagged", "/plain2"] {
        let response = get_request(addr, path).await;
        assert!(response.contains("HTTP/1.1 200"), "{path}: {response}");
    }

    let _ = shutdown_tx.send(());
    let _ = server.await;

    // The app-global hook fires for every request.
    for path in ["/admin/a", "/admin/b", "/tagged", "/plain2"] {
        assert!(log_has(&format!("global:{path}")), "global missing for {path}");
    }
    // The router-scoped hook fires only for the two admin routes.
    assert_eq!(log_count("router"), 2);
    // The route-attribute hook fires only for its single route.
    assert_eq!(log_count("route"), 1);
}
