# 1. Introduction

Tork helps you build HTTP APIs in Rust without giving up control or performance.
It is opinionated about developer experience and unopinionated about your domain.

## What Tork gives you

- **Routers from annotations.** Group handlers in a module, annotate them with
  `#[get]`, `#[post]`, and friends, and Tork builds the router for you.
- **Dependency injection.** Handlers declare what they need (a service, the
  current user, the parsed body) as parameters. Tork resolves each one.
- **Validation and models.** `#[api_model]` turns a struct into a validated,
  documented request or response type.
- **Errors as data.** Every error renders to a consistent JSON body with a status,
  a machine code, a message, optional field details, a trace id, and a timestamp.
- **OpenAPI out of the box.** Routes and models produce an OpenAPI document and a
  documentation page with no extra work.
- **Middleware.** Wrap request handling with built-in or custom layers.

## How it fits together

A Tork application is assembled with the `App` builder and run with `serve`:

```rust
use tork::App;

use my_api::core::app_state::AppState;
use my_api::routers::users;

#[tork::main]
async fn main() -> tork::Result<()> {
    let state = AppState::boot().await?;

    App::new()
        .state(state)
        .include_router(users::router())
        .serve("0.0.0.0:8000")
        .await
}
```

`#[tork::main]` sets up the asynchronous runtime, `state` registers shared
application state, `include_router` mounts a group of routes, and `serve` binds
an address and runs until the process receives a shutdown signal.

The chapters that follow introduce each piece in turn. The next chapter gets a
minimal server running.
