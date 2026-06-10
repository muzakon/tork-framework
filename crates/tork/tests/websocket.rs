//! Confirms a `#[websocket]` handler compiles into a registrable route.

use tork::{App, Router, WebSocket, WsMessage, websocket};

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

#[test]
fn websocket_route_builds() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_echo()))
        .build();
    assert!(app.is_ok(), "the websocket route should register");
}
