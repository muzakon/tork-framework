#![cfg(feature = "redis")]
//! Live Redis tests for the cache store and the raw `Redis` resource.
//!
//! Skipped unless `TORK_TEST_REDIS_URL` points at a Redis server, for example
//! `TORK_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p tork --features redis`.

use std::time::Duration;

use tork::{redis, Cache, Redis};

fn redis_url() -> Option<String> {
    std::env::var("TORK_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

#[tokio::test]
async fn redis_cache_round_trip_ttl_delete_and_clear() {
    let Some(url) = redis_url() else {
        eprintln!("skipping redis test: TORK_TEST_REDIS_URL not set");
        return;
    };

    let cache = Cache::redis(&url).await.expect("connect to redis");
    cache.clear().await.unwrap();

    // Round trip.
    cache.set("k", &"v").await.unwrap();
    assert_eq!(
        cache.get::<String>("k").await.unwrap().as_deref(),
        Some("v")
    );

    // TTL expiry.
    cache
        .set_ttl("temp", &"x", Duration::from_millis(80))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert_eq!(cache.get::<String>("temp").await.unwrap(), None);

    // Delete.
    cache.delete("k").await.unwrap();
    assert_eq!(cache.get::<String>("k").await.unwrap(), None);

    // Clear removes the cache's keys.
    cache.set("a", &1).await.unwrap();
    cache.clear().await.unwrap();
    assert_eq!(cache.get::<i32>("a").await.unwrap(), None);
}

#[tokio::test]
async fn redis_resource_runs_raw_commands_and_shares_with_the_cache() {
    let Some(url) = redis_url() else {
        eprintln!("skipping redis test: TORK_TEST_REDIS_URL not set");
        return;
    };

    let redis = Redis::connect(&url).await.expect("connect to redis");

    // Raw commands through the handle: idempotency-style SET NX.
    let _: () = redis
        .query(redis::cmd("DEL").arg("tork:rawkey"))
        .await
        .unwrap();
    let first: Option<String> = redis
        .query(redis::cmd("SET").arg("tork:rawkey").arg("1").arg("NX"))
        .await
        .unwrap();
    assert!(first.is_some(), "the first SET NX takes the key");
    let second: Option<String> = redis
        .query(redis::cmd("SET").arg("tork:rawkey").arg("2").arg("NX"))
        .await
        .unwrap();
    assert!(second.is_none(), "the second SET NX is a no-op");

    // A cache built on the same handle shares its connection.
    let cache = Cache::from_redis(&redis);
    cache.set("shared", &"value").await.unwrap();
    assert_eq!(
        cache.get::<String>("shared").await.unwrap().as_deref(),
        Some("value")
    );
    cache.clear().await.unwrap();
    let _: () = redis
        .query(redis::cmd("DEL").arg("tork:rawkey"))
        .await
        .unwrap();
}
