//! Tests for preloading: the N+1-free related-row loading and the Preloaded
//! wrapper, against in-memory SQLite.

use tork_orm::prelude::*;

#[derive(Debug, Clone, Model)]
#[table(name = "users")]
struct User {
    #[field(primary_key, auto)]
    id: i64,
    #[field(varchar(length = 50))]
    username: String,
}

#[derive(Debug, Clone, Model, PartialEq)]
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

async fn seed() -> Database {
    let db = Database::connect(":memory:", 1).await.unwrap();
    db.execute(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL)".into(),
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

    // alice: 2 posts (1 published, 1 draft); bob: 1 published; carol: none.
    for username in ["alice", "bob", "carol"] {
        db.execute(
            "INSERT INTO users (username) VALUES (?)".into(),
            vec![Value::Text(username.into())],
        )
        .await
        .unwrap();
    }
    for (user_id, title, published) in [
        (1, "alice-1", true),
        (1, "alice-2", false),
        (2, "bob-1", true),
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
async fn preload_groups_children_onto_parents() {
    let db = seed().await;

    let users = User::query()
        .order_by(User::id.asc())
        .preload(User::posts())
        .all(&db)
        .await
        .unwrap();

    assert_eq!(users.len(), 3);
    // Deref reaches the parent's fields directly.
    assert_eq!(users[0].username, "alice");

    let alice_posts = users[0].get::<Post>();
    assert_eq!(alice_posts.len(), 2);
    assert_eq!(users[1].get::<Post>().len(), 1);
    // carol has no posts; the slice is empty, not missing.
    assert!(users[2].get::<Post>().is_empty());
}

#[tokio::test]
async fn preload_respects_relation_filter_and_order() {
    let db = seed().await;

    let users = User::query()
        .filter(User::username.eq("alice"))
        .preload(
            User::posts()
                .filter(Post::published.eq(true))
                .order_by(Post::id.desc()),
        )
        .all(&db)
        .await
        .unwrap();

    assert_eq!(users.len(), 1);
    let posts = users[0].get::<Post>();
    // Only the published post is loaded.
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].title, "alice-1");
}

#[tokio::test]
async fn preload_runs_one_query_per_relation() {
    let db = seed().await;
    // Count only the statements the preload itself runs (one for parents, one for
    // the relation), independent of how many rows exist.
    let before = db.statement_count();
    let users = User::query()
        .preload(User::posts())
        .all(&db)
        .await
        .unwrap();
    let ran = db.statement_count() - before;
    assert_eq!(ran, 2, "preload should add exactly one query per relation");

    let total_posts: usize = users.iter().map(|u| u.get::<Post>().len()).sum();
    assert_eq!(total_posts, 3);
}

#[tokio::test]
async fn belongs_to_preload_loads_the_parent() {
    let db = seed().await;

    let posts = Post::query()
        .order_by(Post::id.asc())
        .preload(Post::author())
        .all(&db)
        .await
        .unwrap();

    assert_eq!(posts.len(), 3);
    // Each post has exactly one author preloaded.
    let alice = posts[0].get::<User>();
    assert_eq!(alice.len(), 1);
    assert_eq!(alice[0].username, "alice");

    let bob = posts[2].get::<User>();
    assert_eq!(bob[0].username, "bob");
}
