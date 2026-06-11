//! Tests for the QuerySet builder and its terminals, run against a real in-memory
//! SQLite database.

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

/// Creates the schema and seeds a fixed set of users.
async fn seed() -> Database {
    let db = Database::connect(":memory:", 1).await.unwrap();
    db.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL, email TEXT NOT NULL, is_active INTEGER NOT NULL)"
            .into(),
        vec![],
    )
    .await
    .unwrap();

    for (username, email, active) in [
        ("alice", "alice@example.com", true),
        ("bob", "bob@example.com", false),
        ("carol", "carol@example.com", true),
        ("dave", "dave@example.com", true),
    ] {
        db.execute(
            "INSERT INTO users (username, email, is_active) VALUES (?, ?, ?)".into(),
            vec![
                Value::Text(username.into()),
                Value::Text(email.into()),
                Value::Bool(active),
            ],
        )
        .await
        .unwrap();
    }
    db
}

#[tokio::test]
async fn all_returns_every_row() {
    let db = seed().await;
    let users = User::query().all(&db).await.unwrap();
    assert_eq!(users.len(), 4);
    assert_eq!(users[0].username, "alice");
}

#[tokio::test]
async fn filter_applies_an_and_predicate() {
    let db = seed().await;
    let active = User::query()
        .filter(User::is_active.eq(true))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(active.len(), 3);
    assert!(active.iter().all(|u| u.is_active));
}

#[tokio::test]
async fn filter_any_is_an_or_group() {
    let db = seed().await;
    let users = User::query()
        .filter_any([
            User::username.eq("alice"),
            User::username.eq("bob"),
        ])
        .all(&db)
        .await
        .unwrap();
    let mut names: Vec<&str> = users.iter().map(|u| u.username.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["alice", "bob"]);
}

#[tokio::test]
async fn combined_and_or_filters() {
    let db = seed().await;
    // is_active = true AND (username = alice OR username = bob)
    let users = User::query()
        .filter(User::is_active.eq(true))
        .filter_any([
            User::username.eq("alice"),
            User::username.eq("bob"),
        ])
        .all(&db)
        .await
        .unwrap();
    // Only alice is both active and named alice/bob (bob is inactive).
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "alice");
}

#[tokio::test]
async fn filter_not_negates() {
    let db = seed().await;
    let users = User::query()
        .filter_not(User::is_active.eq(true))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "bob");
}

#[tokio::test]
async fn order_by_and_limit() {
    let db = seed().await;
    let users = User::query()
        .order_by(User::id.desc())
        .limit(2)
        .all(&db)
        .await
        .unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].username, "dave");
    assert_eq!(users[1].username, "carol");
}

#[tokio::test]
async fn offset_skips_rows() {
    let db = seed().await;
    let users = User::query()
        .order_by(User::id.asc())
        .limit(2)
        .offset(1)
        .all(&db)
        .await
        .unwrap();
    assert_eq!(users[0].username, "bob");
    assert_eq!(users[1].username, "carol");
}

#[tokio::test]
async fn first_returns_one_or_none() {
    let db = seed().await;
    let found = User::query()
        .filter(User::username.eq("carol"))
        .first(&db)
        .await
        .unwrap();
    assert_eq!(found.unwrap().username, "carol");

    let missing = User::query()
        .filter(User::username.eq("nobody"))
        .first(&db)
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn one_requires_exactly_one_row() {
    let db = seed().await;

    let single = User::query()
        .filter(User::username.eq("alice"))
        .one(&db)
        .await
        .unwrap();
    assert_eq!(single.username, "alice");

    let none = User::query()
        .filter(User::username.eq("nobody"))
        .one(&db)
        .await
        .unwrap_err();
    assert_eq!(none.kind(), ErrorKind::NotFound);

    let many = User::query()
        .filter(User::is_active.eq(true))
        .one(&db)
        .await
        .unwrap_err();
    assert_eq!(many.kind(), ErrorKind::MultipleFound);
}

#[tokio::test]
async fn count_and_exists() {
    let db = seed().await;

    let total = User::query().count(&db).await.unwrap();
    assert_eq!(total, 4);

    let active = User::query()
        .filter(User::is_active.eq(true))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(active, 3);

    assert!(User::query()
        .filter(User::username.eq("alice"))
        .exists(&db)
        .await
        .unwrap());
    assert!(!User::query()
        .filter(User::username.eq("nobody"))
        .exists(&db)
        .await
        .unwrap());
}
