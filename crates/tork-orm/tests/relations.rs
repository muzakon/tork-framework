//! Tests for `#[relations]` and `QuerySet::join`, against in-memory SQLite.

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

#[derive(Debug, Clone, Model)]
#[table(name = "posts")]
struct Post {
    #[field(primary_key, auto)]
    id: i64,
    #[field(foreign_key = User::id)]
    user_id: i64,
    title: String,
    published: bool,
}

#[relations]
impl User {
    #[has_many(Post, foreign_key = Post::user_id)]
    pub fn posts() {}
}

#[relations]
impl Post {
    #[belongs_to(User, foreign_key = Post::user_id)]
    pub fn author() {}
}

#[test]
fn relation_descriptors_carry_the_right_tables() {
    let has_many = User::posts();
    assert_eq!(has_many.kind(), RelationKind::HasMany);
    assert_eq!(has_many.target_table(), "posts");

    let belongs_to = Post::author();
    assert_eq!(belongs_to.kind(), RelationKind::BelongsTo);
    assert_eq!(belongs_to.target_table(), "users");
}

async fn seed() -> Database {
    let db = Database::connect(":memory:", 1).await.unwrap();
    db.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL, is_active INTEGER NOT NULL)".into(),
        vec![],
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, title TEXT NOT NULL, published INTEGER NOT NULL)".into(),
        vec![],
    )
    .await
    .unwrap();

    // alice (active) has one published and one draft post; bob (inactive) has one
    // published post; carol (active) has none.
    for (username, active) in [("alice", true), ("bob", false), ("carol", true)] {
        db.execute(
            "INSERT INTO users (username, is_active) VALUES (?, ?)".into(),
            vec![Value::Text(username.into()), Value::Bool(active)],
        )
        .await
        .unwrap();
    }
    for (user_id, title, published) in [
        (1, "alice-published", true),
        (1, "alice-draft", false),
        (2, "bob-published", true),
    ] {
        db.execute(
            "INSERT INTO posts (user_id, title, published) VALUES (?, ?, ?)".into(),
            vec![
                Value::Int(user_id),
                Value::Text(title.into()),
                Value::Bool(published),
            ],
        )
        .await
        .unwrap();
    }
    db
}

#[tokio::test]
async fn join_filters_parents_by_a_child_column() {
    let db = seed().await;

    // Active users who have at least one published post: only alice (carol has no
    // posts, bob is inactive).
    let users = User::query()
        .join(User::posts())
        .filter(User::is_active.eq(true))
        .filter(Post::published.eq(true))
        .distinct()
        .all(&db)
        .await
        .unwrap();

    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "alice");
}

#[tokio::test]
async fn join_without_distinct_repeats_parent_rows() {
    let db = seed().await;

    // alice has two posts, so joining without distinct yields her row twice.
    let rows = User::query()
        .join(User::posts())
        .filter(User::username.eq("alice"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn belongs_to_join_filters_children_by_parent() {
    let db = seed().await;

    // Posts whose author is active: alice's two posts (bob is inactive).
    let posts = Post::query()
        .join(Post::author())
        .filter(User::is_active.eq(true))
        .all(&db)
        .await
        .unwrap();

    let mut titles: Vec<&str> = posts.iter().map(|p| p.title.as_str()).collect();
    titles.sort_unstable();
    assert_eq!(titles, ["alice-draft", "alice-published"]);
}
