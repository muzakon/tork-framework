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

A Redis store (for sharing a cache across instances) is planned as a feature-gated
backend, configured the same way: `App::cache(Cache::redis(url))`. Because handlers
only see the `Cache` handle, switching stores does not change them.
