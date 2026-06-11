//! Integration tests for recursive dependency injection and authorization.

use bytes::Bytes;
use http_body_util::Full;
use tork::{App, AppInner, Method, ReqBody, StatusCode, box_body};

use my_api::core::app_state::UserStore;
use my_api::routers::users;

async fn app() -> AppInner {
    // Register the resources directly (the live server uses the lifespan instead).
    // The users router only needs the store; configuration is not on its path.
    App::new()
        .state(UserStore::seed())
        .include_router(users::router())
        .build()
        .unwrap()
}

fn request(method: Method, uri: &str, bearer: Option<&str>) -> http::Request<ReqBody> {
    let mut builder = http::Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(box_body(Full::new(Bytes::new()))).unwrap()
}

fn request_json(
    method: Method,
    uri: &str,
    bearer: Option<&str>,
    json: &'static str,
) -> http::Request<ReqBody> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(box_body(Full::new(Bytes::from_static(json.as_bytes()))))
        .unwrap()
}

#[tokio::test]
async fn resolves_user_through_dependency_chain() {
    // UserService -> UserRepository -> State<AppState>
    let response = app()
        .await
        .dispatch(request(Method::GET, "/users/2", None))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn lists_orders_for_authorized_user() {
    let response = app()
        .await
        .dispatch(request(Method::GET, "/users/1/orders/", Some("ada-token")))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn forbids_accessing_another_users_orders() {
    // alan-token authenticates as user 2, but the path targets user 1.
    let response = app()
        .await
        .dispatch(request(Method::GET, "/users/1/orders/", Some("alan-token")))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_unauthenticated_orders_request() {
    let response = app()
        .await
        .dispatch(request(Method::GET, "/users/1/orders/", None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn creates_order_with_valid_body() {
    let body = r#"{"name":"Widget","description":null,"price":9.99,"tax":null}"#;
    let response = app()
        .await
        .dispatch(request_json(
            Method::POST,
            "/users/1/orders",
            Some("ada-token"),
            body,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn rejects_blank_name_via_custom_validator() {
    // Passes min_length but fails the custom `not_blank` validator.
    let body = r#"{"name":"   ","description":null,"price":9.99,"tax":null}"#;
    let response = app()
        .await
        .dispatch(request_json(
            Method::POST,
            "/users/1/orders",
            Some("ada-token"),
            body,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_invalid_order_body() {
    // Blank name and non-positive price both violate the model constraints.
    let body = r#"{"name":"","description":null,"price":0.0,"tax":null}"#;
    let response = app()
        .await
        .dispatch(request_json(
            Method::POST,
            "/users/1/orders",
            Some("ada-token"),
            body,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
