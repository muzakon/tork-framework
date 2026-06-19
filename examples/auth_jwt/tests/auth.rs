//! End-to-end tests for the JWT auth flow, driven in process with `TestClient`.

use std::sync::Arc;

use serde_json::{json, Value};
use tork::testing::TestClient;
use tork::{App, SecretString};

use auth_jwt::config::AuthConfig;
use auth_jwt::routers;
use auth_jwt::users::UserStore;

async fn client() -> TestClient {
    // Build the config directly so the test does not depend on the environment.
    let config = AuthConfig {
        jwt_secret: SecretString::new("test-secret"),
        access_token_ttl_minutes: 30,
    };
    let app = App::new()
        .state(Arc::new(config))
        .state(Arc::new(UserStore::seed()))
        .include_router(routers::router());
    TestClient::new(app.build_test().await.unwrap())
        .await
        .unwrap()
}

/// Logs in and returns (status, token-if-any).
async fn login(client: &TestClient, username: &str, password: &str) -> (u16, Option<String>) {
    let res = client
        .post("/token")
        .json(&json!({ "username": username, "password": password }))
        .send()
        .await
        .unwrap();
    let status = res.status();
    if status == 200 {
        let body: Value = res.json::<Value>().await.unwrap();
        let token = body["access_token"].as_str().unwrap().to_owned();
        (status, Some(token))
    } else {
        (status, None)
    }
}

#[tokio::test]
async fn login_then_read_current_user() {
    let client = client().await;

    let (status, token) = login(&client, "ada", "secret").await;
    assert_eq!(status, 200);
    let token = token.unwrap();

    let res = client
        .get("/users/me")
        .header("authorization", &format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json::<Value>().await.unwrap();
    assert_eq!(body["username"], "ada");
    assert_eq!(body["role"], "admin");

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_token_is_unauthorized() {
    let client = client().await;
    let res = client.get("/users/me").send().await.unwrap();
    assert_eq!(res.status(), 401);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_password_is_unauthorized() {
    let client = client().await;
    let (status, _) = login(&client, "ada", "wrong").await;
    assert_eq!(status, 401);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn scope_protected_route_checks_the_token_scopes() {
    let client = client().await;

    // ada has the `users:write` scope.
    let (_, ada) = login(&client, "ada", "secret").await;
    let res = client
        .get("/admin/overview")
        .header("authorization", &format!("Bearer {}", ada.unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // alan only has `users:read`, so the same route is forbidden.
    let (_, alan) = login(&client, "alan", "secret2").await;
    let res = client
        .get("/admin/overview")
        .header("authorization", &format!("Bearer {}", alan.unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_account_is_forbidden() {
    let client = client().await;

    // dot's credentials are valid, so login succeeds and returns a token.
    let (status, token) = login(&client, "dot", "secret3").await;
    assert_eq!(status, 200);

    // But the account is disabled, so the guard rejects the request.
    let res = client
        .get("/users/me")
        .header("authorization", &format!("Bearer {}", token.unwrap()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    client.shutdown().await.unwrap();
}
