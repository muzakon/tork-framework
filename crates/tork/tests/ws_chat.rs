//! End-to-end WebSocket test: a typed broadcast chat using `receive_valid` and a
//! `Hub`, the connect/disconnect lifecycle hooks, and the idle timeout.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::oneshot;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tork::{websocket, App, Hub, Router, State, WebSocket};

static WS_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn ws_log(entry: impl Into<String>) {
    WS_LOG.lock().unwrap().push(entry.into());
}

#[derive(serde::Deserialize, garde::Validate)]
struct ChatIn {
    #[garde(length(min = 1))]
    message: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ChatMessage {
    from: String,
    text: String,
}

#[websocket("/chat/{room}")]
async fn chat(socket: WebSocket, room: String, hub: State<Hub<ChatMessage>>) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    let room = hub.0.room(room);
    let mut rx = room.subscribe();
    loop {
        tokio::select! {
            incoming = socket.receive_valid::<ChatIn>() => match incoming? {
                Some(input) => {
                    room.broadcast(ChatMessage { from: "user".to_owned(), text: input.message });
                }
                None => break,
            },
            outgoing = rx.recv() => match outgoing {
                Ok(message) => socket.send_json(&message).await?,
                Err(_) => break,
            },
        }
    }
    Ok(())
}

#[websocket("/idle", idle_timeout = "300ms")]
async fn idle(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while socket.recv().await?.is_some() {}
    Ok(())
}

async fn start() -> (std::net::SocketAddr, oneshot::Sender<()>) {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .state(Hub::<ChatMessage>::new())
        .on_ws_connect(|info| async move { ws_log(format!("connect:{}", info.path())) })
        .on_ws_disconnect(
            |info| async move { ws_log(format!("disconnect:{:?}", info.close_code())) },
        )
        .include_router(
            Router::new()
                .route(__tork_route_chat())
                .route(__tork_route_idle()),
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

/// Reads the next message, treating a close, end-of-stream, error, or timeout as
/// "the connection ended".
async fn ended<S>(stream: &mut S) -> bool
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return true,
            Ok(Some(Ok(_))) => continue,
            Err(_) => return false,
        }
    }
}

#[tokio::test]
async fn chat_broadcasts_to_room_and_fires_lifecycle_hooks() {
    WS_LOG.lock().unwrap().clear();
    let (addr, shutdown) = start().await;

    let (mut alice, _) = connect_async(format!("ws://{addr}/chat/general"))
        .await
        .unwrap();
    let (mut bob, _) = connect_async(format!("ws://{addr}/chat/general"))
        .await
        .unwrap();
    // Give both handlers time to accept and subscribe before broadcasting.
    tokio::time::sleep(Duration::from_millis(150)).await;

    alice
        .send(Message::Text(r#"{"message":"hi"}"#.to_owned()))
        .await
        .unwrap();

    let received = tokio::time::timeout(Duration::from_secs(2), bob.next())
        .await
        .expect("bob receives within the timeout")
        .unwrap()
        .unwrap();
    let text = match received {
        Message::Text(text) => text,
        other => panic!("expected a text message, got {other:?}"),
    };
    let message: ChatMessage = serde_json::from_str(&text).unwrap();
    assert_eq!(message.text, "hi");

    alice.close(None).await.unwrap();
    bob.close(None).await.unwrap();
    // The disconnect hook runs on a detached task; give it a moment.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let log = WS_LOG.lock().unwrap();
    assert!(log.iter().any(|e| e == "connect:/chat/general"), "{log:?}");
    assert!(log.iter().any(|e| e.starts_with("disconnect")), "{log:?}");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn invalid_message_closes_the_connection() {
    let (addr, shutdown) = start().await;
    let (mut socket, _) = connect_async(format!("ws://{addr}/chat/room"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    socket
        .send(Message::Text(r#"{"message":""}"#.to_owned()))
        .await
        .unwrap();

    assert!(
        ended(&mut socket).await,
        "an invalid message should close the connection"
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn idle_timeout_closes_an_idle_connection() {
    let (addr, shutdown) = start().await;
    let (mut socket, _) = connect_async(format!("ws://{addr}/idle")).await.unwrap();

    // Send nothing: the server closes the socket after its idle timeout.
    assert!(
        ended(&mut socket).await,
        "an idle connection should time out"
    );
    let _ = shutdown.send(());
}
