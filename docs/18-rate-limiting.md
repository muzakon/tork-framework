# Rate limiting

Tork has built-in rate limiting modeled on NestJS's throttler: define policies once,
then apply them with a `throttle` attribute on routers and routes. Over-limit
requests get `429 Too Many Requests` with a `Retry-After` header.

## Defining policies

Configure the throttler on the app. Give policies names, and optionally mark one as
the global default applied to every route:

```rust
use tork::{App, Throttle};

App::new().throttle(
    Throttle::new()
        .policy("default", 100, 60)   // name, limit, window in seconds
        .policy("strict", 5, 60)
        .default("default"),          // apply "default" to every route by default
);
```

Without `App::throttle(...)`, nothing is limited (the per-route check is a no-op).

## Applying a policy

The `throttle` attribute takes either a **named** policy or an **inline** one:

```rust
use tork::get;

// Use a named policy.
#[get("/feed", throttle = "strict")]
async fn feed() -> tork::Result<Feed> { /* ... */ }

// Or set the limit and window inline (ttl is in seconds).
#[get("/search", throttle(limit = 3, ttl = 60))]
async fn search() -> tork::Result<Results> { /* ... */ }

// Bypass rate limiting for an endpoint.
#[get("/health", throttle = "skip")]
async fn health() -> tork::Result<&'static str> { Ok("ok") }
```

## Router and endpoint levels

A `throttle` on `#[api_router]` applies to every route in it; an endpoint's own
`throttle` **overrides** the router's, and `"skip"` bypasses. Precedence is
**endpoint > router > global default**.

```rust
use tork::{api_router, get};

#[api_router(prefix = "/{user_id}/orders", tags = ["orders"], throttle = "strict")]
pub mod orders_router {
    use super::*;

    // Inherits "strict" from the router.
    #[get("/")]
    pub async fn list(user_id: i64) -> tork::Result<Vec<OrderOut>> { /* ... */ }

    // Overrides with a tighter inline limit, keyed by the user (see below).
    #[get("/recent", throttle(limit = 3, ttl = 60, key = ByUser))]
    pub async fn recent(user_id: i64) -> tork::Result<Vec<OrderOut>> { /* ... */ }
}
```

## Custom keys

Requests are counted per route and per **key** — the client IP by default. To key by
something else (a user id, an API key, a tenant), implement `ThrottleKey` on a unit
type and pass it as `key = ...`. The extractor is async and has the full
`RequestContext`, so it can reuse dependency injection:

```rust
use tork::{RequestContext, ThrottleKey};

struct ByUser;

impl ThrottleKey for ByUser {
    async fn throttle_key(ctx: &RequestContext) -> tork::Result<String> {
        Ok(CurrentUser::from_request(ctx).await?.id.to_string())
    }
}

#[get("/me/orders", throttle(limit = 10, ttl = 60, key = ByUser))]
async fn my_orders(current_user: CurrentUser) -> tork::Result<Vec<OrderOut>> { /* ... */ }
```

The built-in default is `ByIp`.

## Distributed limiting (Redis)

The in-memory store limits each instance independently. To share one limit across
instances, point the throttler at Redis (enable the `redis` feature) — typically the
same connection as the rest of the app:

```rust
# use tork::{App, Cache, Redis, Throttle};
# async fn boot() -> tork::Result<()> {
let redis = Redis::connect("redis://127.0.0.1:6379").await?;
App::new()
    .redis(redis.clone())
    .throttle(Throttle::new().policy("default", 100, 60).default("default").redis(&redis));
# Ok(())
# }
```

Counts are kept in a fixed window using an atomic `INCR`/`EXPIRE`, so every instance
sees the same total.
