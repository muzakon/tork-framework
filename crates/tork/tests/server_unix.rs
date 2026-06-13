//! Serving over a Unix-domain socket (Unix only).
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tork::{get, App};

#[get("/")]
async fn ping() -> tork::Result<&'static str> {
    Ok("pong")
}

#[tokio::test]
async fn serves_an_http_request_over_a_unix_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tork.sock");

    let (ready_tx, ready_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let ready = Arc::new(Mutex::new(Some(ready_tx)));

    let app = App::new().include(ping).on_ready(move |_ctx| {
        let ready = ready.clone();
        async move {
            if let Some(tx) = ready.lock().unwrap().take() {
                let _ = tx.send(());
            }
            Ok(())
        }
    });

    let serve_path = path.clone();
    let handle = tokio::spawn(async move {
        app.serve_unix_with_shutdown(serve_path, async move {
            let _ = shutdown_rx.await;
        })
        .await
    });

    // `on_ready` fires after the socket is bound.
    ready_rx.await.unwrap();

    let mut stream = UnixStream::connect(&path).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf);

    assert!(response.contains("200"), "expected a 200 status: {response}");
    assert!(response.contains("pong"), "expected the handler body: {response}");

    let _ = shutdown_tx.send(());
    let _ = handle.await;
}
