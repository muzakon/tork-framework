//! Integration tests for the in-process test client's WebSocket and SSE support.

use futures_util::stream;
use serde_json::json;
use tork::testing::TestClient;
use tork::{
    App, BearerToken, Router, Sse, WebSocket, WsCloseCode, WsMessage, api_model, sse, websocket,
};

#[websocket("/ws")]
async fn ws_echo(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while let Some(message) = socket.recv().await? {
        match message {
            WsMessage::Text(text) => socket.send_text(text).await?,
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

#[websocket("/hello")]
async fn ws_hello(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    socket
        .send_json(&json!({ "msg": "Hello WebSocket" }))
        .await?;
    socket.close(WsCloseCode::NormalClosure, "done").await?;
    Ok(())
}

#[websocket("/guarded")]
async fn ws_guarded(socket: WebSocket, _token: BearerToken) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    socket.send_text("ok").await?;
    Ok(())
}

#[tokio::test]
async fn websocket_echo_in_process() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_ws_echo()))
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let mut ws = client.websocket("/ws").connect().await.unwrap();
    ws.send_text("hello").await.unwrap();
    assert_eq!(ws.receive_text().await.unwrap(), "hello");
    ws.close().await.unwrap();
}

#[tokio::test]
async fn websocket_typed_json_message() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_ws_hello()))
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let mut ws = client.websocket("/hello").connect().await.unwrap();
    let data = ws.receive_json::<serde_json::Value>().await.unwrap();
    assert_eq!(data, json!({ "msg": "Hello WebSocket" }));
}

#[tokio::test]
async fn websocket_upgrade_rejected_without_auth() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_ws_guarded()))
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    // The handler requires a bearer token; without one the upgrade is rejected
    // before it is accepted, so connect returns an error.
    let result = client.websocket("/guarded").connect().await;
    assert!(result.is_err(), "expected the upgrade to be rejected");
}

#[api_model]
struct Tick {
    n: i64,
}

#[sse("/events", event = "tick", response_model = Tick)]
async fn events() -> tork::Result<Sse<Tick>> {
    let items = stream::iter(vec![
        Ok::<_, tork::Error>(Tick { n: 1 }),
        Ok(Tick { n: 2 }),
    ]);
    Ok(Sse::new(items).no_heartbeat())
}

#[tokio::test]
async fn sse_stream_reads_events() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_events()))
        .build_test()
        .await
        .unwrap();
    let client = TestClient::new(app).await.unwrap();

    let mut stream = client.get("/events").sse().await.unwrap();

    let first = stream.next_event().await.unwrap().expect("first event");
    assert_eq!(first.event(), Some("tick"));
    assert_eq!(first.json::<Tick>().unwrap().n, 1);

    let second = stream.next_event().await.unwrap().expect("second event");
    assert_eq!(second.json::<Tick>().unwrap().n, 2);

    assert!(stream.next_event().await.unwrap().is_none(), "stream should end");
}
