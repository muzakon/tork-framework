//! Integration tests for the `throttle` rate-limiting attribute: per-route limits,
//! router defaults, endpoint overrides/skip, the global default, and custom keys.

use tork::testing::TestClient;
use tork::{api_model, api_router, get, App, RequestContext, Throttle, ThrottleKey};

/// A deterministic test key: rate-limit by the `x-client` header (so tests do not
/// depend on a peer IP).
struct ByClient;

impl ThrottleKey for ByClient {
    async fn throttle_key(ctx: &RequestContext) -> tork::Result<String> {
        Ok(ctx
            .headers()
            .get("x-client")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("anon")
            .to_string())
    }
}

#[api_model]
struct Pong {
    pong: bool,
}

fn pong() -> Pong {
    Pong { pong: true }
}

#[get("/inline", throttle(limit = 2, ttl = 60, key = ByClient))]
async fn inline_limited() -> tork::Result<Pong> {
    Ok(pong())
}

#[get("/plain")]
async fn plain() -> tork::Result<Pong> {
    Ok(pong())
}

#[get("/multi", throttle = ["loose", "tight"])]
async fn multi() -> tork::Result<Pong> {
    Ok(pong())
}

#[get("/slide", throttle(limit = 2, ttl = 60))]
async fn slide() -> tork::Result<Pong> {
    Ok(pong())
}

#[api_router(prefix = "/r", throttle(limit = 2, ttl = 60, key = ByClient))]
mod limited_router {
    use super::*;

    #[get("/inherit")]
    pub async fn inherit() -> tork::Result<Pong> {
        Ok(pong())
    }

    #[get("/skip", throttle = "skip")]
    pub async fn skip() -> tork::Result<Pong> {
        Ok(pong())
    }

    #[get("/tight", throttle(limit = 1, ttl = 60, key = ByClient))]
    pub async fn tight() -> tork::Result<Pong> {
        Ok(pong())
    }
}

async fn client(app: App) -> TestClient {
    TestClient::new(app.build_test().await.unwrap())
        .await
        .unwrap()
}

async fn hit(client: &TestClient, path: &str, who: &str) -> u16 {
    client
        .get(path)
        .header("x-client", who)
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn inline_limit_blocks_after_the_threshold() {
    let client = client(App::new().throttle(Throttle::new()).include(inline_limited)).await;

    assert_eq!(hit(&client, "/inline", "a").await, 200);
    assert_eq!(hit(&client, "/inline", "a").await, 200);

    // The third request from the same client is blocked with 429 + Retry-After.
    let blocked = client
        .get("/inline")
        .header("x-client", "a")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert!(blocked.headers().get("retry-after").is_some());

    // A different client has its own budget.
    assert_eq!(hit(&client, "/inline", "b").await, 200);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn router_default_applies_skip_bypasses_and_override_tightens() {
    let client = client(
        App::new()
            .throttle(Throttle::new())
            .include_router(limited_router::router()),
    )
    .await;

    // Inherits the router default (limit 2).
    assert_eq!(hit(&client, "/r/inherit", "a").await, 200);
    assert_eq!(hit(&client, "/r/inherit", "a").await, 200);
    assert_eq!(hit(&client, "/r/inherit", "a").await, 429);

    // `throttle = "skip"` bypasses entirely.
    for _ in 0..5 {
        assert_eq!(hit(&client, "/r/skip", "a").await, 200);
    }

    // The endpoint override is tighter (limit 1).
    assert_eq!(hit(&client, "/r/tight", "a").await, 200);
    assert_eq!(hit(&client, "/r/tight", "a").await, 429);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn global_default_applies_to_unannotated_routes() {
    // A global default of 2/window, applied to a route with no `throttle` attr.
    let client = client(
        App::new()
            .throttle(Throttle::new().policy("default", 2, 60).default("default"))
            .include(plain),
    )
    .await;

    // No custom key, so all in-process requests share the IP key ("unknown").
    assert_eq!(client.get("/plain").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/plain").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/plain").send().await.unwrap().status(), 429);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn multiple_policies_apply_the_tightest() {
    // The route is subject to both "loose" (10) and "tight" (2); the tighter wins.
    let client = client(
        App::new()
            .throttle(
                Throttle::new()
                    .policy("loose", 10, 60)
                    .policy("tight", 2, 60),
            )
            .include(multi),
    )
    .await;

    assert_eq!(client.get("/multi").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/multi").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/multi").send().await.unwrap().status(), 429);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn sliding_window_enforces_the_limit() {
    let client = client(
        App::new()
            .throttle(Throttle::new().sliding())
            .include(slide),
    )
    .await;

    assert_eq!(client.get("/slide").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/slide").send().await.unwrap().status(), 200);
    assert_eq!(client.get("/slide").send().await.unwrap().status(), 429);

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn without_a_throttler_nothing_is_limited() {
    // The route declares a tight inline limit, but no `App::throttle(...)` means
    // the check is a no-op.
    let client = client(App::new().include(inline_limited)).await;

    for _ in 0..5 {
        assert_eq!(hit(&client, "/inline", "a").await, 200);
    }

    client.shutdown().await.unwrap();
}
