# Tork

A FastAPI-style backend web framework for Rust, built directly on
[Hyper](https://hyper.rs) and [Tokio](https://tokio.rs). Tork provides
annotation-based routers, dependency injection, and OpenAPI generation, with an
emphasis on a small, modular core and security-conscious defaults.

> Status: Phase 1 (framework architecture). The ORM and CLI are separate,
> later-phase efforts.

## Workspace layout

```
framework/
  crates/
    tork           Facade crate (the only crate applications depend on)
    tork-core      Runtime: server, routing, dependency injection, errors
    tork-macros    Procedural macros
    tork-openapi   OpenAPI document generation and the docs UI
  examples/
    my_api         A complete example application
```

## Quick start

```rust
use tork::{App, OpenApi};

use my_api::core::app_state::AppState;
use my_api::routers::users;

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .include_router(users::router())
        .openapi(
            OpenApi::new()
                .title("My API")
                .version("1.0.0")
                .json("/openapi.json")
                .docs("/docs"),
        )
        .serve("0.0.0.0:8000")
        .await
}
```

Define routes by annotating an inline module:

```rust
use tork::{api_router, get};

#[api_router(prefix = "/users", tags = ["users"])]
pub mod users_router {
    use super::*;

    #[get("/{user_id}", response_model = UserOut, summary = "Get user by id")]
    pub async fn get_user(user_id: i64, service: UserService) -> tork::Result<UserOut> {
        service.get_user(user_id).await
    }
}
```

A handler parameter whose name matches a `{placeholder}` in the route path is a
path parameter; every other parameter is resolved through dependency injection.

Define a dependency by annotating its `resolve` function:

```rust
use tork::{BearerToken, dependency};

#[tork::dependency]
impl CurrentUser {
    pub async fn resolve(token: BearerToken, users: UserRepository) -> tork::Result<Self> {
        // Each parameter is itself resolved through dependency injection.
    }
}
```

## Features

- Annotation-based routers with prefix and tag composition (`#[api_router]`,
  `#[get]`, `#[post]`, ...).
- Dependency injection through the `FromRequest` trait, with recursive
  resolution (`#[tork::dependency]`).
- Built-in extractors: `State<S>`, `BearerToken`, and `Json<T>` (with a request
  body size cap).
- Structured errors with a security-conscious response model (5xx detail is
  never leaked to clients).
- OpenAPI document generation at `/openapi.json` and a documentation UI at
  `/docs`.
- HTTP/1 and HTTP/2 via Hyper, with graceful shutdown on `SIGINT`/`SIGTERM`.

## Building and running

```sh
cargo build --workspace
cargo test --workspace
cargo run -p my_api
```

Then visit `http://127.0.0.1:8000/docs` for the API documentation.
