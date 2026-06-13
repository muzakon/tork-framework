//! Confirms `#[derive(FormModel)]` binds multipart fields and validates them.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tork::{
    api_model, box_body, post, App, FileBytes, FormModel, FromRequest, Method, Multipart,
    PathParams, ReqBody, RequestContext, Router, StateMap, StatusCode,
};

#[derive(FormModel)]
struct CreateFileForm {
    #[file]
    file: FileBytes,
    #[field(min_length = 6)]
    token: String,
    note: Option<String>,
}

fn ctx(content_type: &str, body: &[u8]) -> RequestContext {
    let head = http::Request::builder()
        .header("content-type", content_type)
        .body(())
        .unwrap()
        .into_parts()
        .0;
    let body = box_body(Full::new(Bytes::copy_from_slice(body)));
    RequestContext::new(head, PathParams::new(), Arc::new(StateMap::new()), body)
}

#[tokio::test]
async fn model_binds_file_and_text_fields() {
    let body = "--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhello\r\n\
                --X\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nsecret-token\r\n--X--\r\n";
    let ctx = ctx("multipart/form-data; boundary=X", body.as_bytes());

    let form = Multipart::<CreateFileForm>::from_request(&ctx)
        .await
        .unwrap()
        .into_inner();

    assert_eq!(form.file.bytes(), b"hello");
    assert_eq!(form.file.filename(), Some("a.txt"));
    assert_eq!(form.token, "secret-token");
    assert_eq!(form.note, None);
}

#[tokio::test]
async fn model_validation_rejects_a_short_token() {
    let body =
        "--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhi\r\n\
                --X\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nshort\r\n--X--\r\n";
    let ctx = ctx("multipart/form-data; boundary=X", body.as_bytes());

    let error = Multipart::<CreateFileForm>::from_request(&ctx)
        .await
        .err()
        .expect("validation should fail");
    assert_eq!(error.code(), "VALIDATION_ERROR");
}

#[api_model]
struct FileInfoOut {
    size: usize,
    token: String,
}

#[post("/files")]
async fn create_file(#[file] file: FileBytes, #[form] token: String) -> tork::Result<FileInfoOut> {
    Ok(FileInfoOut {
        size: file.len(),
        token,
    })
}

#[tokio::test]
async fn parameter_based_upload_binds_file_and_form() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_create_file()))
        .build()
        .unwrap();

    let body =
        "--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhello\r\n\
                --X\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nabc123\r\n--X--\r\n";
    let request: http::Request<ReqBody> = http::Request::builder()
        .method(Method::POST)
        .uri("/files")
        .header("content-type", "multipart/form-data; boundary=X")
        .body(box_body(Full::new(Bytes::from(body))))
        .unwrap();

    let response = app.dispatch(request).await;
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["size"], 5);
    assert_eq!(json["token"], "abc123");
}

#[post("/avatar")]
async fn upload_avatar(#[file(max_size = "3B")] avatar: FileBytes) -> tork::Result<i64> {
    Ok(avatar.len() as i64)
}

#[tokio::test]
async fn oversize_file_is_rejected() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_upload_avatar()))
        .build()
        .unwrap();

    let body = "--X\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a.bin\"\r\n\r\nhello\r\n--X--\r\n";
    let request: http::Request<ReqBody> = http::Request::builder()
        .method(Method::POST)
        .uri("/avatar")
        .header("content-type", "multipart/form-data; boundary=X")
        .body(box_body(Full::new(Bytes::from(body))))
        .unwrap();

    let response = app.dispatch(request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
