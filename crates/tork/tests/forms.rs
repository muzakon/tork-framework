//! Confirms `#[derive(FormModel)]` binds multipart fields and validates them.

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use tork::{
    FileBytes, FormModel, FromRequest, Multipart, PathParams, RequestContext, StateMap, box_body,
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
    let body = "--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nhi\r\n\
                --X\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nshort\r\n--X--\r\n";
    let ctx = ctx("multipart/form-data; boundary=X", body.as_bytes());

    let error = Multipart::<CreateFileForm>::from_request(&ctx)
        .await
        .err()
        .expect("validation should fail");
    assert_eq!(error.code(), "VALIDATION_ERROR");
}
