//! Confirms an `#[sse]` endpoint streams `text/event-stream` events over TCP.

use std::sync::{Arc, Mutex};

use futures_util::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tork::{App, Router, Sse, get, sse};

#[sse("/stream", event = "tick")]
async fn stream_numbers() -> tork::Result<Sse<serde_json::Value>> {
    let items = stream::iter(vec![
        Ok::<_, tork::Error>(serde_json::json!({ "n": 1 })),
        Ok(serde_json::json!({ "n": 2 })),
    ]);
    // No heartbeat keeps the test output deterministic.
    Ok(Sse::new(items).no_heartbeat())
}

#[get("/plain")]
async fn plain() -> tork::Result<i64> {
    Ok(1)
}

async fn read_response(addr: std::net::SocketAddr, path: &str) -> String {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn sse_endpoint_streams_event_stream() {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .include_router(
            Router::new()
                .route(__tork_route_stream_numbers())
                .route(__tork_route_plain()),
        )
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
    let response = read_response(addr, "/stream").await;

    assert!(response.contains("HTTP/1.1 200"), "status: {response}");
    assert!(
        response.contains("content-type: text/event-stream"),
        "content type: {response}"
    );
    assert!(response.contains("cache-control: no-cache"), "cache: {response}");
    assert!(
        response.contains("event: tick\r\n") || response.contains("event: tick\n"),
        "event name: {response}"
    );
    assert!(response.contains("data: {\"n\":1}"), "first event: {response}");
    assert!(response.contains("data: {\"n\":2}"), "second event: {response}");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}
