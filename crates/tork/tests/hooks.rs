//! End-to-end test of the request hooks, a typed exception handler, and the
//! panic boundary over a real TCP connection.

use std::sync::{Arc, Mutex};

use tork::testing::TestClient;
use tork::{get, App, IntoResponse, Router, StatusCode};

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

/// Sends one `GET` request and returns the buffered response body.
async fn get_request(client: &TestClient, path: &str) -> (u16, String) {
    let response = client.get(path).send().await.unwrap();
    let status = response.status();
    let body = response.text().unwrap();
    (status, body)
}

#[tokio::test]
async fn hooks_observe_requests_handle_typed_errors_and_catch_panics() {
    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));

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
        .include_router(
            Router::new()
                .route(__tork_route_ok_handler())
                .route(__tork_route_db_handler())
                .route(__tork_route_boom_handler()),
        );

    let client = TestClient::serve(app).bind_random_port().await.unwrap();

    let (status, body) = get_request(&client, "/ok").await;
    assert_eq!(status, 200, "ok response body: {body}");

    let (status, body) = get_request(&client, "/db").await;
    assert_eq!(status, 418, "db response body: {body}");

    let (status, body) = get_request(&client, "/boom").await;
    assert_eq!(status, 500, "boom response body: {body}");

    let (status, body) = get_request(&client, "/missing").await;
    assert_eq!(status, 404, "missing response body: {body}");

    client.shutdown().await.unwrap();

    // Observability hooks ran for the successful request.
    assert!(contains(&recorder, "request:/ok"), "{recorder:?}");
    assert!(contains(&recorder, "response:200"), "{recorder:?}");
    // The typed error fired on_error (non-validation) and was mapped by the handler.
    assert!(
        contains(&recorder, "error:SERVICE_UNAVAILABLE"),
        "{recorder:?}"
    );
    assert!(contains(&recorder, "handled:db"), "{recorder:?}");
    // The panic was caught and observed.
    assert!(contains(&recorder, "panic:kaboom"), "{recorder:?}");
    // The missing route fired on_error.
    assert!(contains(&recorder, "error:NOT_FOUND"), "{recorder:?}");
}
