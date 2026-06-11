# 16. Project structure

Tork does not impose a layout, but the example application follows a structure
that scales well. It separates HTTP concerns from business logic and data access.

```
my_api/
  Cargo.toml
  src/
    lib.rs            Declares the modules below
    main.rs           The #[tork::main] entrypoint that wires everything together

    core/
      mod.rs
      app_state.rs    Shared application state and its boot() function
      auth.rs         The CurrentUser dependency and authorization helpers

    models/
      mod.rs
      user.rs         #[api_model] request and response types
      order.rs

    repositories/
      mod.rs
      user_repository.rs   Data access, resolved from application state

    services/
      mod.rs
      user_service.rs      Business logic, resolved from repositories
      order_service.rs

    routers/
      mod.rs
      users.rs        #[api_router] modules and a router() function
      orders.rs
```

## How the layers relate

- **Models** (`models/`) are the data shapes that cross the API boundary. They are
  `#[api_model]` types: validated on the way in, serialized on the way out, and
  reflected in the OpenAPI document.
- **Repositories** (`repositories/`) handle data access. They are dependencies
  that resolve from `State<AppState>`, so a handler never touches the state map
  directly.
- **Services** (`services/`) hold business logic and depend on repositories. A
  handler asks for a service and calls a method on it.
- **Routers** (`routers/`) declare the HTTP surface. Handlers stay thin: they
  extract what they need, call a service, and return a result.
- **Core** (`core/`) holds cross-cutting pieces: application state, bootstrapping,
  and authentication.

## Wiring it together

`main.rs` is where the pieces meet. It boots the state, registers middleware,
mounts routers, configures OpenAPI, and serves:

```rust
use tork::middleware::{Compression, Cors, RequestId, Trace};
use tork::{App, OpenApi};

use my_api::core::app_state::AppState;
use my_api::routers::users;

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .middleware(RequestId::new())
        .middleware(Trace::new())
        .middleware(Cors::new().allow_origin("*").allow_methods(["GET", "POST"]))
        .middleware(Compression::new().gzip().minimum_size(256))
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

This is the same shape you saw in chapter 1, now with each part explained. From
here you can grow the application by adding models, repositories, services, and
routers, while `main.rs` stays a thin assembly point.
