//! Integration tests for recursive dependency injection and authorization,
//! written with the in-process `TestClient`.

use serde_json::json;
use tork::testing::TestClient;
use tork::{App, StatusCode};

use my_api::core::app_state::UserStore;
use my_api::routers::users;

/// Builds a client over the users router with a seeded store.
///
/// The users router only needs the store; configuration is not on its path.
async fn client() -> TestClient {
    TestClient::builder(App::new().state(UserStore::seed()).include_router(users::router()))
        .build()
        .await
        .unwrap()
}

#[tokio::test]
async fn resolves_user_through_dependency_chain() {
    // UserService -> UserRepository -> the store.
    let response = client().await.get("/users/2").send().await.unwrap();
    assert_eq!(response.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn lists_orders_for_authorized_user() {
    let response = client()
        .await
        .get("/users/1/orders/")
        .header("authorization", "Bearer ada-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn forbids_accessing_another_users_orders() {
    // alan-token authenticates as user 2, but the path targets user 1.
    let response = client()
        .await
        .get("/users/1/orders/")
        .header("authorization", "Bearer alan-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rejects_unauthenticated_orders_request() {
    let response = client().await.get("/users/1/orders/").send().await.unwrap();
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn creates_order_with_valid_body() {
    let response = client()
        .await
        .post("/users/1/orders")
        .header("authorization", "Bearer ada-token")
        .json(&json!({ "name": "Widget", "description": null, "price": 9.99, "tax": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status_code(), StatusCode::CREATED);
}

#[tokio::test]
async fn rejects_blank_name_via_custom_validator() {
    // Passes min_length but fails the custom `not_blank` validator.
    let response = client()
        .await
        .post("/users/1/orders")
        .header("authorization", "Bearer ada-token")
        .json(&json!({ "name": "   ", "description": null, "price": 9.99, "tax": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rejects_invalid_order_body() {
    // Blank name and non-positive price both violate the model constraints.
    let response = client()
        .await
        .post("/users/1/orders")
        .header("authorization", "Bearer ada-token")
        .json(&json!({ "name": "", "description": null, "price": 0.0, "tax": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
}
