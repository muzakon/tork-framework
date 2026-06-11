//! End-to-end upload tests over a real TCP connection: a model-based multipart
//! upload, a parameter-based upload, a urlencoded `Form` login, and a validation
//! rejection. The request bodies are built by hand and sent through a socket, so
//! the whole server pipeline (parsing, binding, validation) is exercised.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tork::{App, FileBytes, Form, FormModel, Multipart, Router, api_model, post};

#[api_model]
struct UploadOut {
    size: usize,
    token: String,
}

#[derive(FormModel)]
struct AvatarForm {
    #[file]
    avatar: FileBytes,
    #[field(min_length = 6)]
    token: String,
}

#[post("/model")]
async fn upload_model(form: Multipart<AvatarForm>) -> tork::Result<UploadOut> {
    let form = form.into_inner();
    Ok(UploadOut {
        size: form.avatar.len(),
        token: form.token,
    })
}

#[post("/params")]
async fn upload_params(
    #[file] avatar: FileBytes,
    #[form] token: String,
) -> tork::Result<UploadOut> {
    Ok(UploadOut {
        size: avatar.len(),
        token,
    })
}

#[api_model]
struct LoginForm {
    username: String,
    password: String,
}

#[api_model]
struct LoginOut {
    username: String,
}

#[post("/login")]
async fn login(form: Form<LoginForm>) -> tork::Result<LoginOut> {
    let form = form.into_inner();
    Ok(LoginOut {
        username: form.username,
    })
}

/// Sends a POST request with the given content type and body, returning the full
/// raw response (status line, headers, and body).
async fn post_request(
    addr: std::net::SocketAddr,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> String {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let header = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(header.as_bytes()).await.unwrap();
    socket.write_all(body).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    response
}

/// Returns the response body (everything after the header terminator).
fn response_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or("")
}

#[tokio::test]
async fn uploads_bind_over_tcp() {
    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .include_router(
            Router::new()
                .route(__tork_route_upload_model())
                .route(__tork_route_upload_params())
                .route(__tork_route_login()),
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

    let multipart = "--B\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\n\r\nhello!\r\n\
                     --B\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nsecret-token\r\n--B--\r\n";

    // Model-based multipart upload.
    let response = post_request(addr, "/model", "multipart/form-data; boundary=B", multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 200"), "model status: {response}");
    let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
    assert_eq!(body["size"], 6);
    assert_eq!(body["token"], "secret-token");

    // Parameter-based multipart upload.
    let response = post_request(addr, "/params", "multipart/form-data; boundary=B", multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 200"), "params status: {response}");
    let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
    assert_eq!(body["size"], 6);
    assert_eq!(body["token"], "secret-token");

    // Urlencoded form login.
    let response = post_request(
        addr,
        "/login",
        "application/x-www-form-urlencoded",
        b"username=alice&password=hunter2",
    )
    .await;
    assert!(response.contains("HTTP/1.1 200"), "login status: {response}");
    let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
    assert_eq!(body["username"], "alice");

    // A short token fails validation with 422 Unprocessable Entity.
    let short = "--B\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\n\r\nhello!\r\n\
                 --B\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nshort\r\n--B--\r\n";
    let response = post_request(addr, "/model", "multipart/form-data; boundary=B", short.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 422"), "validation status: {response}");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}
