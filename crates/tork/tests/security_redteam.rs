//! Adversarial (red-team) security regression suite.
//!
//! Each test fires a real attack at a realistic Tork app and asserts the *secure*
//! outcome, so the suite doubles as regression coverage for the framework's
//! security guarantees. Attacks that can only be observed end-to-end (slowloris,
//! oversized frames, leak inspection) live here; precise unit-level checks (e.g. the
//! SSE field-injection sanitizer) live next to the code they cover.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tork::testing::TestClient;
use tork::{
    App, BearerToken, FormModel, Multipart, UploadConfig, Valid, WebSocket, WsMessage, api_model,
    get, post, websocket,
};

/// A trivial route used as a target for connection-level attacks.
#[get("/")]
async fn ping() -> tork::Result<&'static str> {
    Ok("pong")
}

/// A handler whose internal error carries a secret, to prove it never reaches the
/// client.
#[get("/boom")]
async fn boom() -> tork::Result<&'static str> {
    Err(tork::Error::internal(
        "DB auth failed for postgres://admin:hunter2@10.0.0.5/prod (file /etc/tork/secret.key)",
    ))
}

/// A handler guarded by a bearer token, to prove a rejected credential is not
/// echoed back.
#[get("/secret")]
async fn secret(_token: BearerToken) -> tork::Result<&'static str> {
    Ok("ok")
}

#[derive(FormModel)]
struct Tiny {
    #[allow(dead_code)]
    a: String,
}

/// A multipart route, used to prove a flood of tiny fields is capped.
#[post("/flood")]
async fn flood(_form: Multipart<Tiny>) -> tork::Result<&'static str> {
    Ok("ok")
}

#[api_model]
struct Item {
    #[allow(dead_code)]
    name: String,
}

/// A JSON-body route, used to prove deeply nested payloads are rejected.
#[post("/items")]
async fn create_item(_item: Valid<Item>) -> tork::Result<&'static str> {
    Ok("ok")
}

/// A WebSocket echo route, used to prove oversized messages are rejected.
#[websocket("/ws")]
async fn ws_echo(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while let Some(message) = socket.recv().await? {
        if let WsMessage::Binary(bytes) = message {
            socket.send_binary(bytes).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Slowloris: a client that opens a connection and never finishes its headers
// must be dropped once the header-read timeout elapses, not held forever.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn slowloris_slow_header_client_is_dropped() {
    let app = App::new()
        .include(ping)
        .header_read_timeout(Duration::from_millis(300));
    let client = TestClient::serve(app).bind_random_port().await.unwrap();
    let addr = client.local_addr().expect("a bound address");

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Send a partial request head and deliberately never send the terminating
    // blank line, the classic slowloris hold.
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: target\r\n")
        .await
        .unwrap();

    // With the header-read timeout, the server closes (or 408s then closes) the
    // connection, so the read completes. Without it, the connection would hang and
    // our outer 5s deadline would fire instead.
    let started = tokio::time::Instant::now();
    let mut buf = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    let elapsed = started.elapsed();
    assert!(
        outcome.is_ok(),
        "server kept the slow-header connection open past the read timeout"
    );
    // It was held for roughly the timeout window (not closed instantly for an
    // unrelated reason), proving the deadline is what dropped it.
    assert!(
        elapsed >= Duration::from_millis(200),
        "connection closed in {elapsed:?}, before the header-read timeout could fire"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Idle timeout: a connection with no read/write activity is dropped once the
// configured idle timeout elapses.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn idle_timeout_closes_an_inactive_connection() {
    let app = App::new()
        .include(ping)
        .idle_timeout(Duration::from_millis(300));
    let client = TestClient::serve(app).bind_random_port().await.unwrap();
    let addr = client.local_addr().expect("a bound address");

    // Connect and then stay completely silent — no request bytes at all.
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let started = tokio::time::Instant::now();
    let mut buf = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    let elapsed = started.elapsed();

    assert!(outcome.is_ok(), "idle connection was not closed in time");
    assert!(
        elapsed >= Duration::from_millis(200),
        "connection closed in {elapsed:?}, before the idle timeout could fire"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Information disclosure: a 5xx must not leak the internal error detail, source
// chain, secrets, or file paths to the client.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn server_error_does_not_leak_internal_detail() {
    let client = TestClient::serve(App::new().include(boom))
        .bind_random_port()
        .await
        .unwrap();

    let response = client.get("/boom").send().await.unwrap();
    assert_eq!(response.status(), 500);

    let body = response.text().unwrap();
    assert!(!body.contains("hunter2"), "leaked secret: {body}");
    assert!(!body.contains("postgres://"), "leaked DSN: {body}");
    assert!(!body.contains("/etc/tork"), "leaked path: {body}");
    assert!(
        body.contains("Internal server error"),
        "expected the generic message, got: {body}"
    );
    // Error responses must not be cached by proxies/browsers.
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "no-store"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Credential echo: a rejected Authorization value must never appear in the error
// body (no "invalid token: <token>" leaks).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn rejected_authorization_is_not_echoed() {
    let client = TestClient::serve(App::new().include(secret))
        .bind_random_port()
        .await
        .unwrap();

    // Wrong scheme: carries a secret-looking credential the framework must not echo.
    let response = client
        .get("/secret")
        .header("authorization", "Basic c3VwZXItc2VjcmV0LWNyZWQ=")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    let body = response.text().unwrap();
    assert!(
        !body.contains("c3VwZXItc2VjcmV0LWNyZWQ="),
        "echoed the supplied credential: {body}"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Multipart amplification: a flood of tiny fields (under the byte-size limits)
// must be rejected once the field-count cap is exceeded.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn multipart_field_flood_is_rejected() {
    let app = App::new()
        .upload_config(UploadConfig::new().max_fields(5))
        .include(flood);
    let client = TestClient::serve(app).bind_random_port().await.unwrap();

    let mut form = client.post("/flood").multipart();
    for i in 0..50 {
        form = form.text(&format!("f{i}"), "x");
    }
    let response = form.send().await.unwrap();
    assert_eq!(response.status(), 422);
    assert!(
        response.text().unwrap().contains("TOO_MANY_FIELDS"),
        "expected a TOO_MANY_FIELDS rejection"
    );

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// WebSocket memory bound: a message larger than the default cap must not be
// buffered and echoed; the connection ends instead.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn websocket_oversized_message_is_rejected_by_default() {
    let client = TestClient::serve(App::new().include(ws_echo))
        .bind_random_port()
        .await
        .unwrap();
    let mut ws = client.websocket("/ws").connect().await.unwrap();

    // 2 MiB exceeds the 1 MiB default message/frame cap. The server rejects the
    // frame and closes the connection, which surfaces either as a write error
    // (connection reset mid-send) here ...
    if ws.send_binary(vec![0u8; 2 * 1024 * 1024]).await.is_err() {
        client.shutdown().await.unwrap();
        return;
    }

    // ... or, if the whole frame was buffered before the reset, as a close/error
    // on receive. What must never happen is the oversized message being echoed.
    let outcome = tokio::time::timeout(Duration::from_secs(5), ws.receive())
        .await
        .expect("server should respond within 5s");
    match outcome {
        Ok(Some(WsMessage::Binary(_))) => panic!("server echoed an oversized message"),
        _ => { /* a close frame, end-of-stream, or error are all acceptable */ }
    }

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// JSON depth bomb: a deeply nested payload must be rejected by the pre-scan
// before a recursive parser can overflow the stack.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn deeply_nested_json_is_rejected_before_parsing() {
    let client = TestClient::serve(App::new().include(create_item))
        .bind_random_port()
        .await
        .unwrap();

    // 5000 levels of nesting, far past the 128-level guard — the shape that
    // overflows a recursive-descent JSON parser.
    let payload = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
    let response = client
        .post("/items")
        .header("content-type", "application/json")
        .bytes(payload)
        .send()
        .await
        .unwrap();

    // Rejected cleanly with a 400, not a crash or stack overflow.
    assert_eq!(response.status(), 400);

    // The server is still alive afterwards.
    let alive = client.get("/items").send().await.unwrap();
    assert!(alive.status() >= 400, "server should still respond");

    client.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Configurable body cap: a body larger than max_request_body_size is rejected.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn configured_max_request_body_size_is_enforced() {
    let client = TestClient::serve(App::new().max_request_body_size(64).include(create_item))
        .bind_random_port()
        .await
        .unwrap();

    // A JSON body well over the 64-byte cap is rejected before it is buffered.
    let payload = format!(r#"{{"name":"{}"}}"#, "x".repeat(200));
    let response = client
        .post("/items")
        .header("content-type", "application/json")
        .bytes(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(
        response.text().unwrap().contains("too large"),
        "expected a body-too-large rejection"
    );

    client.shutdown().await.unwrap();
}
