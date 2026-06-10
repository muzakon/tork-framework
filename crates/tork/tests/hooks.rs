//! End-to-end test of the request hooks, a typed exception handler, and the
//! panic boundary over a real TCP connection.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tork::{App, IntoResponse, Router, StatusCode, get};

/// A user error mapped to `503` by default, recovered by an exception handler.
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

#[get("/ok")]
async fn ok_handler() -> tork::Result<i64> {
    Ok(1)
}

#[get("/db")]
async fn db_handler() -> tork::Result<i64> {
    // The `?` converts `DbError` into `tork::Error`, carrying the typed source.
    let outcome: std::result::Result<i64, DbError> = Err(DbError::Timeout);
    Ok(outcome?)
}

#[get("/boom")]
async fn boom_handler() -> tork::Result<i64> {
    panic!("kaboom");
}

/// A shared log of the labels each hook recorded.
type Recorder = Arc<Mutex<Vec<String>>>;

fn push(recorder: &Recorder, label: impl Into<String>) {
    recorder.lock().unwrap().push(label.into());
}

fn contains(recorder: &Recorder, label: &str) -> bool {
    recorder.lock().unwrap().iter().any(|entry| entry == label)
}

/// Sends one `GET` request on a fresh connection and returns the raw response.
async fn get_request(addr: std::net::SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn hooks_observe_requests_handle_typed_errors_and_catch_panics() {
    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));

    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let request_rec = recorder.clone();
    let response_rec = recorder.clone();
    let error_rec = recorder.clone();
    let panic_rec = recorder.clone();
    let handler_rec = recorder.clone();

    let app = App::new()
        .catch_panics()
        .on_request(move |event| {
            let rec = request_rec.clone();
            let label = format!("request:{}", event.path());
            async move { push(&rec, label) }
        })
        .on_response(move |event| {
            let rec = response_rec.clone();
            let label = format!("response:{}", event.status().as_u16());
            async move { push(&rec, label) }
        })
        .on_error(move |event| {
            let rec = error_rec.clone();
            let label = format!("error:{}", event.code());
            async move { push(&rec, label) }
        })
        .on_panic(move |event| {
            let rec = panic_rec.clone();
            let label = format!("panic:{}", event.message());
            async move { push(&rec, label) }
        })
        .exception_handler::<DbError, _, _>(move |error, _ctx| {
            let rec = handler_rec.clone();
            async move {
                assert_eq!(error, DbError::Timeout);
                push(&rec, "handled:db");
                StatusCode::IM_A_TEAPOT.into_response()
            }
        })
        .on_ready(move |ctx| {
            let sender = sender.clone();
            async move {
                if let Some(tx) = sender.lock().unwrap().take() {
                    let _ = tx.send(ctx.addr());
                }
                Ok(())
            }
        })
        .include_router(
            Router::new()
                .route(__tork_route_ok_handler())
                .route(__tork_route_db_handler())
                .route(__tork_route_boom_handler()),
        );

    let server = tokio::spawn(app.serve_with_shutdown("127.0.0.1:0", async move {
        let _ = shutdown_rx.await;
    }));

    let addr = addr_rx.await.unwrap();

    let ok = get_request(addr, "/ok").await;
    assert!(ok.contains("HTTP/1.1 200"), "ok response: {ok}");

    let db = get_request(addr, "/db").await;
    assert!(db.contains("HTTP/1.1 418"), "db response: {db}");

    let boom = get_request(addr, "/boom").await;
    assert!(boom.contains("HTTP/1.1 500"), "boom response: {boom}");

    let missing = get_request(addr, "/missing").await;
    assert!(missing.contains("HTTP/1.1 404"), "missing response: {missing}");

    let _ = shutdown_tx.send(());
    let _ = server.await;

    // Observability hooks ran for the successful request.
    assert!(contains(&recorder, "request:/ok"), "{recorder:?}");
    assert!(contains(&recorder, "response:200"), "{recorder:?}");
    // The typed error fired on_error (non-validation) and was mapped by the handler.
    assert!(contains(&recorder, "error:SERVICE_UNAVAILABLE"), "{recorder:?}");
    assert!(contains(&recorder, "handled:db"), "{recorder:?}");
    // The panic was caught and observed.
    assert!(contains(&recorder, "panic:kaboom"), "{recorder:?}");
    // The missing route fired on_error.
    assert!(contains(&recorder, "error:NOT_FOUND"), "{recorder:?}");
}
