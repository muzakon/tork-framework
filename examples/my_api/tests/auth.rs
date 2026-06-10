//! Integration tests for recursive dependency injection and authorization.

use bytes::Bytes;
use http_body_util::Full;
use tork::{App, AppInner, Method, ReqBody, StatusCode, box_body};

use my_api::core::app_state::AppState;
use my_api::routers::users;

async fn app() -> AppInner {
    let state = AppState::boot().await.unwrap();
    App::new()
        .state(state)
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
