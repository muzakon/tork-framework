//! Tests for aggregates, group_by/having, projection, and all_as, against
//! in-memory SQLite. This exercises the full target query DX.

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
    view_count: i64,
}

#[relations]
impl User {
    #[has_many(Post, foreign_key = Post::user_id)]
    pub fn posts() {}
}

#[derive(Debug, QueryResult)]
struct UserPostStats {
    user_id: i64,
    username: String,
    post_count: i64,
    total_views: i64,
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
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, title TEXT NOT NULL, view_count INTEGER NOT NULL)".into(),
        vec![],
    )
    .await
    .unwrap();

    for username in ["alice", "bob", "carol"] {
        db.execute(
            "INSERT INTO users (username, is_active) VALUES (?, ?)".into(),
            vec![Value::Text(username.into()), Value::Bool(true)],
        )
        .await
        .unwrap();
    }
    // alice: 4 posts (views 10,20,30,40); bob: 2 posts (views 5,15); carol: none.
    for (user_id, views) in [
        (1, 10),
        (1, 20),
        (1, 30),
        (1, 40),
        (2, 5),
        (2, 15),
    ] {
        db.execute(
            "INSERT INTO posts (user_id, title, view_count) VALUES (?, ?, ?)".into(),
            vec![
                Value::Int(user_id),
                Value::Text("post".into()),
                Value::Int(views),
            ],
        )
        .await
        .unwrap();
    }
    db
}

#[tokio::test]
async fn count_aggregate_in_projection() {
    let db = seed().await;
    // Total number of posts via COUNT(*) selected into a one-field DTO.
    #[derive(QueryResult)]
    struct Total {
        total: i64,
    }
    let stats = Post::query()
        .select((Post::id.count().as_("total"),))
        .all_as::<Total>(&db)
        .await
        .unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].total, 6);
}

#[tokio::test]
async fn group_by_having_order_and_projection() {
    let db = seed().await;

    // Per active user: post_count and total_views, only users with more than 2
    // posts, ordered by total views descending. Only alice (4 posts) qualifies.
    let stats = User::query()
        .select((
            User::id.as_("user_id"),
            User::username,
            Post::id.count().as_("post_count"),
            Post::view_count.sum().as_("total_views"),
        ))
        .join(User::posts())
        .filter(User::is_active.eq(true))
        .group_by((User::id, User::username))
        .having(Post::id.count().gt(2))
        .order_by(Post::view_count.sum().desc())
        .all_as::<UserPostStats>(&db)
        .await
        .unwrap();

    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].user_id, 1);
    assert_eq!(stats[0].username, "alice");
    assert_eq!(stats[0].post_count, 4);
    assert_eq!(stats[0].total_views, 100);
}

#[tokio::test]
async fn group_by_without_having_returns_all_groups() {
    let db = seed().await;

    let stats = User::query()
        .select((
            User::id.as_("user_id"),
            User::username,
            Post::id.count().as_("post_count"),
            Post::view_count.sum().as_("total_views"),
        ))
        .join(User::posts())
        .group_by((User::id, User::username))
        .order_by(User::id.asc())
        .all_as::<UserPostStats>(&db)
        .await
        .unwrap();

    // alice and bob have posts; carol has none, so the inner join drops her.
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].post_count, 4);
    assert_eq!(stats[0].total_views, 100);
    assert_eq!(stats[1].username, "bob");
    assert_eq!(stats[1].post_count, 2);
    assert_eq!(stats[1].total_views, 20);
}

#[tokio::test]
async fn min_max_avg_aggregates() {
    let db = seed().await;
    #[derive(QueryResult)]
    struct Bounds {
        lowest: i64,
        highest: i64,
    }
    let bounds = Post::query()
        .select((
            Post::view_count.min().as_("lowest"),
            Post::view_count.max().as_("highest"),
        ))
        .all_as::<Bounds>(&db)
        .await
        .unwrap();
    assert_eq!(bounds[0].lowest, 5);
    assert_eq!(bounds[0].highest, 40);
}
