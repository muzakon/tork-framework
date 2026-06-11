//! Integration with the Tork web framework: a Database registered as a resource,
//! injected into handlers as Arc<Database>, with ORM errors bridged to HTTP
//! statuses. Driven in-process through the TestClient.

use std::sync::Arc;

use serde_json::json;
use tork::testing::TestClient;
use tork::{get, App};
use tork_orm::prelude::*;

#[derive(Debug, Clone, Model)]
#[table(name = "users")]
struct User {
    #[field(primary_key, auto)]
    id: i64,
    #[field(varchar(length = 50))]
    username: String,
    is_active: bool,
}

fn new_user(username: &str, is_active: bool) -> User {
    User {
        id: 0,
        username: username.into(),
        is_active,
    }
}

// `Arc<Database>` is injected via the framework's blanket resource extractor; a
// failed `one()` becomes a 404 through the ORM-to-framework error bridge.
#[get("/users/{id}")]
async fn get_user(id: i64, db: Arc<Database>) -> tork::Result<serde_json::Value> {
    let user = User::query().filter(User::id.eq(id)).one(&db).await?;
    Ok(json!({ "id": user.id, "username": user.username }))
}

#[get("/users")]
async fn count_users(db: Arc<Database>) -> tork::Result<serde_json::Value> {
    let total = User::query().filter(User::is_active.eq(true)).count(&db).await?;
    Ok(json!({ "active": total }))
}

/// Builds a database with a seeded schema and returns it ready to register.
async fn seeded_database() -> Database {
    let db = Database::connect(":memory:", 1).await.unwrap();
    db.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL, is_active INTEGER NOT NULL)".into(),
        vec![],
    )
    .await
    .unwrap();
    User::create(&db, &new_user("alice", true)).await.unwrap();
    User::create(&db, &new_user("bob", false)).await.unwrap();
    db
}

async fn client() -> TestClient {
    let db = seeded_database().await;
    let app = App::new()
        .state(Arc::new(db))
        .include(get_user)
        .include(count_users)
        .build_test()
        .await
        .unwrap();
    TestClient::new(app).await.unwrap()
}

#[tokio::test]
async fn injected_database_runs_a_query() {
    let client = client().await;
    let response = client.get("/users/1").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["username"], "alice");
}

#[tokio::test]
async fn aggregate_through_injected_database() {
    let client = client().await;
    let response = client.get("/users").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["active"], 1);
}

#[tokio::test]
async fn not_found_error_bridges_to_404() {
    let client = client().await;
    // No user with id 999, so `one()` returns NotFound, which the bridge maps to
    // an HTTP 404.
    let response = client.get("/users/999").send().await.unwrap();
    assert_eq!(response.status(), 404);
}
