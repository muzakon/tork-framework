//! Confirms a `#[websocket]` handler upgrades and echoes over a real connection,
//! and that a failing dependency rejects the upgrade with an HTTP status.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::oneshot;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::error::Error as WsClientError;
use tork::{App, BearerToken, Router, WebSocket, WsMessage, api_model, websocket};

#[websocket("/ws")]
async fn echo(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while let Some(message) = socket.recv().await? {
        match message {
            WsMessage::Text(text) => socket.send_text(text).await?,
            WsMessage::Binary(bytes) => socket.send_binary(bytes).await?,
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// A guarded endpoint: the bearer-token dependency fails without a token.
#[websocket("/guarded")]
async fn guarded(socket: WebSocket, _token: BearerToken) -> tork::Result<()> {
    let _ = socket.accept().await?;
    Ok(())
}

#[test]
fn websocket_route_builds() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_echo()))
        .build();
    assert!(app.is_ok(), "the websocket route should register");
}

#[api_model]
struct WsIn {
    text: String,
}

#[api_model]
struct WsOut {
    text: String,
}

#[websocket("/typed", incoming = WsIn, outgoing = WsOut)]
async fn typed(socket: WebSocket) -> tork::Result<()> {
    let _ = socket.accept().await?;
    Ok(())
}

#[test]
fn websocket_records_asyncapi_metadata() {
    let route = __tork_route_typed();
    assert!(route.meta().websocket, "should be marked as a websocket route");
    assert!(route.meta().ws_incoming.is_some(), "incoming schema recorded");
    assert!(route.meta().ws_outgoing.is_some(), "outgoing schema recorded");
}

/// Starts the server and returns the bound address plus a shutdown handle.
async fn start() -> (std::net::SocketAddr, oneshot::Sender<()>) {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .include_router(
            Router::new()
                .route(__tork_route_echo())
                .route(__tork_route_guarded()),
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

    tokio::spawn(app.serve_with_shutdown("127.0.0.1:0", async move {
        let _ = shutdown_rx.await;
    }));

    (addr_rx.await.unwrap(), shutdown_tx)
}

#[tokio::test]
async fn echoes_text_and_binary_over_a_real_connection() {
    let (addr, shutdown) = start().await;

    let (mut socket, _response) = connect_async(format!("ws://{addr}/ws")).await.unwrap();

    socket.send(Message::Text("hello".into())).await.unwrap();
    let reply = socket.next().await.unwrap().unwrap();
    assert_eq!(reply, Message::Text("hello".into()));

    socket.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
    let reply = socket.next().await.unwrap().unwrap();
    assert_eq!(reply, Message::Binary(vec![1, 2, 3]));

    socket.close(None).await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test]
async fn upgrade_is_rejected_when_a_dependency_fails() {
    let (addr, shutdown) = start().await;

    // No Authorization header: the bearer-token dependency fails before accept.
    let result = connect_async(format!("ws://{addr}/guarded")).await;

    match result {
        Err(WsClientError::Http(response)) => {
            assert_eq!(response.status(), 401, "expected an unauthorized rejection");
        }
        other => panic!("expected an HTTP rejection, got {other:?}"),
    }

    let _ = shutdown.send(());
}

#[tokio::test]
async fn websocket_origin_header_is_present_in_handshake() {
    let (addr, shutdown) = start().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("ws://{addr}/ws"))
        .header("Host", addr.to_string())
        .header("Origin", "https://evil.example.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .unwrap();

    let result = connect_async(request).await;

    // Currently the framework accepts any origin (no validation)
    // This test documents the current behavior - a proper implementation
    // would reject untrusted origins with 403
    match result {
        Ok((mut socket, _response)) => {
            socket.close(None).await.unwrap();
        }
        Err(WsClientError::Http(response)) => {
            panic!("unexpected HTTP error: {}", response.status());
        }
        other => panic!("expected successful connection, got {other:?}"),
    }

    let _ = shutdown.send(());
}
