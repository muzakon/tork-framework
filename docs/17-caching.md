# Caching

Tork ships a small, store-agnostic cache with TTL support. The same handler code
works whether values are kept in memory or, later, in an external store, so you can
start simple and swap the backend without rewriting anything.

## Enabling the cache

Configure a cache on the app, then inject it where you need it. The default backend
is an in-memory store:

```rust
use tork::{App, Cache};

App::new().cache(Cache::in_memory());
```

Without this call, injecting a `Cache` fails — caching is opt-in.

## Reading and writing

A `Cache` is injected like any other resource. Values are serialized to JSON, so
any `serde` type can be cached:

```rust
use tork::{get, Cache};

#[get("/users/{id}")]
async fn get_user(id: i64, users: UserService, cache: Cache) -> tork::Result<User> {
    let key = format!("user:{id}");

    // Cache-aside in one call: a hit returns immediately; a miss runs the closure
    // once and caches the result.
    let user = cache
        .get_or_set(&key, None, || async { users.load(id).await })
        .await?;

    Ok(user)
}
```

The lower-level methods are also available:

```rust
cache.set("greeting", &"hello").await?;            // default TTL
let greeting: Option<String> = cache.get("greeting").await?;
cache.delete("greeting").await?;
cache.clear().await?;                              // drop everything
```

`get::<T>` returns `Ok(None)` when the key is absent or has expired.

## Time-to-live

A cache entry can expire after a duration. Set a per-entry TTL with `set_ttl`, or a
default TTL for the whole cache with `default_ttl`:

```rust
use std::time::Duration;

// Expire this entry after 30 seconds.
cache.set_ttl("token", &token, Duration::from_secs(30)).await?;

// Apply a default TTL to every plain `set`.
let cache = Cache::in_memory().default_ttl(Duration::from_secs(60));
```

A zero duration means "never expire". With no TTL configured, entries live until the
in-memory store evicts the least-recently-used ones at its capacity limit.

## Swapping the store

The backend is a `CacheStore` — a small trait with `get` / `set` / `delete` /
`clear` over byte values. The built-in `MemoryStore` is the default; you can supply
your own with `Cache::new(store)`:

```rust
let cache = Cache::new(MyCustomStore::new());
```

Because handlers only see the `Cache` handle, switching stores does not change them.

## Redis

Enable the `redis` feature to use Redis:

```toml
tork = { version = "...", features = ["redis"] }
```

The quickest path is a cache backed by Redis (shared across instances, so a value
cached by one process is visible to the others):

```rust
use tork::{App, Cache};

# async fn boot() -> tork::Result<()> {
let cache = Cache::redis("redis://127.0.0.1:6379").await?;
App::new().cache(cache);
# Ok(())
# }
```

Keys are namespaced with a `tork:` prefix, so `clear()` removes only this cache's
keys (it scans and deletes by prefix, never flushing the whole database). The URL
typically comes from configuration — see [Settings](13-settings.md).

### Redis as a resource

Redis is more than a cache — idempotency keys, atomic counters, distributed locks,
Lua scripts, pub/sub. Tork does not wrap any of that. It manages the connection and
makes a `Redis` handle injectable; for everything else you use the re-exported
`redis` client directly. Register a connection, then inject `Redis` anywhere:

```rust
use tork::{get, redis, Redis};

#[get("/charge/{id}")]
async fn charge(id: String, redis: Redis) -> tork::Result<&'static str> {
    // Idempotency: only the first request for this id takes the key.
    let taken: Option<String> = redis
        .query(redis::cmd("SET").arg(format!("idem:{id}")).arg("1").arg("NX").arg("EX").arg(60))
        .await?;
    if taken.is_none() {
        return Ok("already processed");
    }
    // ... do the work ...
    Ok("charged")
}
```

Build and register the connection with `App::redis`:

```rust
# use tork::{App, Cache, Redis};
# async fn boot() -> tork::Result<()> {
let redis = Redis::connect("redis://127.0.0.1:6379").await?;
App::new()
    .redis(redis.clone())            // raw Redis, injectable as `Redis`
    .cache(Cache::from_redis(&redis)); // a cache sharing the same connection
# Ok(())
# }
```

`redis.connection()` hands you the underlying client connection for the full API
(`redis::Script` for Lua, `redis::pipe` for pipelines, and so on). Because the
`redis` crate is re-exported as `tork::redis`, you do not add it as a separate
dependency.
