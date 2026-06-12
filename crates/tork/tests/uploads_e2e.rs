//! End-to-end upload tests over a real TCP connection: a model-based multipart
//! upload, a parameter-based upload, a urlencoded `Form` login, and a validation
//! rejection. The request bodies are built by hand and sent through a socket, so
//! the whole server pipeline (parsing, binding, validation) is exercised.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::fs;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tork::{App, Form, FormModel, Multipart, Router, UploadFile, FileBytes, api_model, post};
use tempfile::TempDir;

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

struct UploadState {
    upload_dir: PathBuf,
    outside_path: PathBuf,
}

#[post("/upload")]
async fn upload_save(mut file: UploadFile, state: Arc<UploadState>) -> tork::Result<serde_json::Value> {
    file.save_to_dir(&state.upload_dir, "safe.txt").await?;
    Ok(serde_json::json!({ "status": "ok" }))
}

#[post("/upload-invalid-path")]
async fn upload_invalid_path(mut file: UploadFile, state: Arc<UploadState>) -> tork::Result<serde_json::Value> {
    file.save_to(&state.outside_path).await?;
    Ok(serde_json::json!({ "status": "ok" }))
}

#[tokio::test]
async fn upload_file_save_to_dir_is_safe_and_save_to_rejects_invalid_paths() {
    let temp_dir = TempDir::new().unwrap();
    let upload_dir = temp_dir.path().join("uploads");
    let outside_path = temp_dir.path().join("../outside.txt");
    fs::create_dir_all(&upload_dir).unwrap();

    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .state(Arc::new(UploadState {
            upload_dir: upload_dir.clone(),
            outside_path,
        }))
        .include_router(
            Router::new()
                .route(__tork_route_upload_save())
                .route(__tork_route_upload_invalid_path()),
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

    let multipart = "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhello!\r\n--B--\r\n";

    let response = post_request(addr, "/upload", "multipart/form-data; boundary=B", multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 200"), "safe upload status: {response}");

    let response = post_request(addr, "/upload-invalid-path", "multipart/form-data; boundary=B", multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 400"), "invalid upload path should fail: {response}");

    assert!(upload_dir.join("safe.txt").exists());

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

struct SymlinkState {
    symlink_path: PathBuf,
}

#[post("/upload-symlink")]
async fn upload_symlink(mut file: UploadFile, state: Arc<SymlinkState>) -> tork::Result<serde_json::Value> {
    file.save_to(&state.symlink_path).await?;
    Ok(serde_json::json!({ "status": "ok" }))
}

#[tokio::test]
async fn upload_file_save_to_rejects_symlink_attack() {
    let temp_dir = TempDir::new().unwrap();
    let upload_dir = temp_dir.path().join("uploads");
    fs::create_dir_all(&upload_dir).unwrap();

    let target_file = temp_dir.path().join("target.txt");
    fs::write(&target_file, "original").unwrap();

    let symlink_path = upload_dir.join("malicious.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target_file, &symlink_path).unwrap();

    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .state(Arc::new(SymlinkState {
            symlink_path: symlink_path.clone(),
        }))
        .include_router(Router::new().route(__tork_route_upload_symlink()))
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

    let multipart = "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nmalicious\r\n--B--\r\n";

    let response = post_request(addr, "/upload-symlink", "multipart/form-data; boundary=B", multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 400"), "symlink upload should fail: {response}");

    let content = fs::read_to_string(&target_file).unwrap();
    assert_eq!(content, "original");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}

#[tokio::test]
async fn multipart_temp_files_cleaned_up_on_parse_error() {
    let temp_dir = TempDir::new().unwrap();
    let upload_dir = temp_dir.path().join("uploads");
    fs::create_dir_all(&upload_dir).unwrap();

    #[post("/upload-error")]
    async fn upload_error(_file: UploadFile) -> tork::Result<serde_json::Value> {
        Ok(serde_json::json!({ "status": "ok" }))
    }

    let (addr_tx, addr_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(addr_tx)));

    let app = App::new()
        .include_router(Router::new().route(__tork_route_upload_error()))
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

    let truncated_multipart = "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhello";

    let response = post_request(addr, "/upload-error", "multipart/form-data; boundary=B", truncated_multipart.as_bytes()).await;
    assert!(response.contains("HTTP/1.1 400") || response.contains("HTTP/1.1 500"), "truncated upload should fail: {response}");

    let temp_files: Vec<_> = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("multer"))
        .collect();
    assert!(temp_files.is_empty(), "temp files should be cleaned up: {:?}", temp_files);

    let _ = shutdown_tx.send(());
    let _ = server.await;
}
