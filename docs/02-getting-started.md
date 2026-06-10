# 2. Getting started

This chapter builds the smallest useful Tork service: one route that returns JSON.

## Dependencies

An application depends on `tork`. When you use `#[api_model]` you also depend on
`garde`, because the generated validation code refers to it directly. `serde` and
`schemars` do not need to be added; the macros route to them through `tork`.

```toml
[dependencies]
tork = "0.1"
garde = "0.23"   # only needed if you use #[api_model]
```

## A minimal application

A handler is an `async fn` that returns `tork::Result<T>`, where `T` serializes to
JSON. Group handlers in a module annotated with `#[api_router]`:

```rust
// src/routers/health.rs
use tork::{api_router, get};

#[api_router(prefix = "/health", tags = ["health"])]
pub mod health_router {
    use super::*;

    #[get("/", summary = "Liveness check")]
    pub async fn live() -> tork::Result<&'static str> {
        Ok("ok")
    }
}

pub fn router() -> tork::Router {
    health_router::router()
}
```

The `#[api_router]` macro reads the annotated functions in the module and
generates a `router()` function inside it. The free `router()` function below the
module is a small convenience so callers write `health::router()`.

## Wiring the entrypoint

```rust
// src/main.rs
use tork::App;

use my_api::routers::health;

#[tork::main]
async fn main() -> tork::Result<()> {
    App::new()
        .include_router(health::router())
        .serve("0.0.0.0:8000")
        .await
}
```

Run it and call the route:

```sh
cargo run
curl http://127.0.0.1:8000/health
# ok
```

## What just happened

- `#[get("/", ...)]` registered a `GET` route at the router prefix `/health` plus
  the local path `/`, which normalizes to `/health`.
- The handler returned `Ok("ok")`. Tork serialized the value as the response body.
- `serve` ran the server until you stopped it with Ctrl-C, draining in-flight
  requests on the way out.

The next chapter covers routing in depth: path parameters, methods, and nesting.
