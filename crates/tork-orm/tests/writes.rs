//! Tests for the write operations: create (with RETURNING), bulk_create, save,
//! and bulk update/delete, against in-memory SQLite.

use tork_orm::prelude::*;

#[derive(Debug, Clone, Model, PartialEq)]
#[table(name = "users")]
struct User {
    #[field(primary_key, auto)]
    id: i64,
    #[field(varchar(length = 50))]
    username: String,
    email: String,
    is_active: bool,
}

async fn empty_db() -> Database {
    let db = Database::connect(":memory:", 1).await.unwrap();
    db.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL, email TEXT NOT NULL, is_active INTEGER NOT NULL)"
            .into(),
        vec![],
    )
    .await
    .unwrap();
    db
}

fn new_user(username: &str) -> User {
    User {
        id: 0,
        username: username.into(),
        email: format!("{username}@example.com"),
        is_active: true,
    }
}

#[tokio::test]
async fn create_returns_the_persisted_row_with_generated_id() {
    let db = empty_db().await;
    let stored = User::create(&db, &new_user("alice")).await.unwrap();
    // The database assigned the id even though the input had id 0.
    assert_eq!(stored.id, 1);
    assert_eq!(stored.username, "alice");

    let second = User::create(&db, &new_user("bob")).await.unwrap();
    assert_eq!(second.id, 2);
}

#[tokio::test]
async fn bulk_create_inserts_many() {
    let db = empty_db().await;
    let inserted = User::bulk_create(
        &db,
        &[new_user("alice"), new_user("bob"), new_user("carol")],
    )
    .await
    .unwrap();
    assert_eq!(inserted, 3);
    assert_eq!(User::query().count(&db).await.unwrap(), 3);
}

#[tokio::test]
async fn bulk_create_of_nothing_is_a_noop() {
    let db = empty_db().await;
    let inserted = User::bulk_create(&db, &[]).await.unwrap();
    assert_eq!(inserted, 0);
}

#[tokio::test]
async fn save_writes_back_current_field_values() {
    let db = empty_db().await;
    let mut stored = User::create(&db, &new_user("alice")).await.unwrap();

    stored.email = "alice@new.example.com".into();
    stored.is_active = false;
    let changed = stored.save(&db).await.unwrap();
    assert_eq!(changed, 1);

    let reloaded = User::query()
        .filter(User::id.eq(stored.id))
        .one(&db)
        .await
        .unwrap();
    assert_eq!(reloaded.email, "alice@new.example.com");
    assert!(!reloaded.is_active);
    // The auto primary key is untouched by save.
    assert_eq!(reloaded.id, stored.id);
}

#[tokio::test]
async fn query_update_sets_columns_on_matching_rows() {
    let db = empty_db().await;
    User::bulk_create(
        &db,
        &[new_user("alice"), new_user("bob"), new_user("carol")],
    )
    .await
    .unwrap();

    let changed = User::query()
        .filter(User::username.ne("bob"))
        .update(&db, [User::is_active.set(false)])
        .await
        .unwrap();
    assert_eq!(changed, 2);

    let active = User::query()
        .filter(User::is_active.eq(true))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].username, "bob");
}

#[tokio::test]
async fn query_delete_removes_matching_rows() {
    let db = empty_db().await;
    User::bulk_create(
        &db,
        &[new_user("alice"), new_user("bob"), new_user("carol")],
    )
    .await
    .unwrap();

    let removed = User::query()
        .filter(User::username.eq("bob"))
        .delete(&db)
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(User::query().count(&db).await.unwrap(), 2);

    let all_removed = User::query().delete(&db).await.unwrap();
    assert_eq!(all_removed, 2);
    assert_eq!(User::query().count(&db).await.unwrap(), 0);
}
