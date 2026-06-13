//! Integration tests for the cache: `App::cache(...)` registration and injecting
//! a `Cache` into handlers, with values persisting across requests.

use tork::testing::TestClient;
use tork::{api_model, get, App, Cache};

#[api_model]
struct Fetched {
    value: Option<String>,
}

#[get("/set/{key}/{value}")]
async fn set_value(key: String, value: String, cache: Cache) -> tork::Result<Fetched> {
    cache.set(&key, &value).await?;
    Ok(Fetched { value: Some(value) })
}

#[get("/get/{key}")]
async fn get_value(key: String, cache: Cache) -> tork::Result<Fetched> {
    let value: Option<String> = cache.get(&key).await?;
    Ok(Fetched { value })
}

#[tokio::test]
async fn cache_persists_across_requests() {
    let client = TestClient::new(
        App::new()
            .cache(Cache::in_memory())
            .include(set_value)
            .include(get_value)
            .build_test()
            .await
            .unwrap(),
    )
    .await
    .unwrap();

    // Before setting anything, the key is a miss.
    let before = client.get("/get/greeting").send().await.unwrap();
    assert_eq!(before.status(), 200);
    assert!(before.text().unwrap().contains("\"value\":null"));

    // Set on one request, read it back on another — the cache is shared state.
    let set = client.get("/set/greeting/hello").send().await.unwrap();
    assert_eq!(set.status(), 200);

    let after = client.get("/get/greeting").send().await.unwrap();
    assert_eq!(after.status(), 200);
    assert!(after.text().unwrap().contains("\"hello\""));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn injecting_cache_without_configuring_it_errors() {
    // No App::cache(...) call, so resolving the Cache fails with a 500.
    let client = TestClient::new(App::new().include(get_value).build_test().await.unwrap())
        .await
        .unwrap();

    let response = client.get("/get/x").send().await.unwrap();
    assert_eq!(response.status(), 500);

    client.shutdown().await.unwrap();
}
