//! Integration tests for the route macros, exercised through the facade crate.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use serde::Serialize;
use tork::{App, Method, ReqBody, Router, StatusCode, box_body, get};

#[derive(Serialize, schemars::JsonSchema)]
struct Pong {
    id: i64,
    ok: bool,
}

#[get("/ping/{id}", response_model = Pong, summary = "Ping with an id")]
async fn ping(id: i64) -> tork::Result<Pong> {
    Ok(Pong { id, ok: id == 1 })
}

fn request(method: Method, uri: &str) -> http::Request<ReqBody> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .body(box_body(Full::new(Bytes::new())))
        .unwrap()
}

#[test]
fn route_metadata_is_populated() {
    let route = __tork_route_ping();
    assert_eq!(route.method(), &Method::GET);
    assert_eq!(route.path(), "/ping/{id}");
    assert_eq!(route.meta().summary.as_deref(), Some("Ping with an id"));
    assert_eq!(route.meta().status_code, StatusCode::OK);
    assert!(route.meta().response_model.is_some());
}

#[tokio::test]
async fn generated_handler_parses_path_and_responds() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_ping()))
        .build()
        .unwrap();

    let response = app.dispatch(request(Method::GET, "/ping/1")).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("\"id\":1"), "unexpected body: {text}");
    assert!(text.contains("\"ok\":true"), "unexpected body: {text}");
}

#[tokio::test]
async fn invalid_path_parameter_is_rejected() {
    let app = App::new()
        .include_router(Router::new().route(__tork_route_ping()))
        .build()
        .unwrap();

    let response = app.dispatch(request(Method::GET, "/ping/not-a-number")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
