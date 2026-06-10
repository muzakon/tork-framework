# 3. Routing

Routes are declared with method macros inside a module annotated with
`#[api_router]`.

## Method macros

`#[get]`, `#[post]`, `#[put]`, `#[patch]`, and `#[delete]` each take a path and a
set of optional attributes:

```rust
use tork::{api_router, get, post};
use crate::models::user::UserOut;

#[api_router(prefix = "/users", tags = ["users"])]
pub mod users_router {
    use super::*;

    #[get("/{user_id}", response_model = UserOut, summary = "Get user by id")]
    pub async fn get_user(user_id: i64) -> tork::Result<UserOut> {
        // ...
    }
}
```

Supported attributes:

- The path string, given first and positionally.
- `response_model = Type`: the type used for the response schema. When it differs
  from the handler's return type, the returned value is converted into it.
- `summary = "..."` and `description = "..."`: documentation text.
- `status_code = 201`: the success status (defaults to `200`).
- `tags = ["a", "b"]`: documentation tags for this route.

## Path parameters

A handler parameter whose name matches a `{placeholder}` in the path is a path
parameter. It is parsed from the URL using the type's `FromStr` implementation:

```rust
#[get("/{user_id}", summary = "Get user by id")]
pub async fn get_user(user_id: i64) -> tork::Result<UserOut> {
    // user_id is parsed from the path segment
}
```

If the segment cannot be parsed into the declared type, the request is rejected
with `422 Unprocessable Entity`. Every other handler parameter is resolved through
dependency injection, which the next chapter covers.

## Prefixes and tags

`#[api_router(prefix = "...", tags = [...])]` applies a prefix and a tag set to
every route in the module. The prefix is joined with each route's local path, and
a trailing slash is dropped (except for the root path).

## Nesting routers

A router can include another router. Prefixes compose and tags are merged:

```rust
// src/routers/orders.rs
#[api_router(prefix = "/{user_id}/orders", tags = ["orders"])]
pub mod orders_router {
    use super::*;

    #[get("/", response_model = Vec<OrderOut>, summary = "List orders for a user")]
    pub async fn list_user_orders(user_id: i64) -> tork::Result<Vec<OrderOut>> {
        // ...
    }
}

// src/routers/users.rs
pub fn router() -> tork::Router {
    users_router::router().include(crate::routers::orders::orders_router::router())
}
```

Mounting the users router now exposes:

- `GET /users/{user_id}`
- `GET /users/{user_id}/orders`

A path parameter declared in an enclosing prefix (`{user_id}` above) is available
to nested handlers as an ordinary parameter.

## Methods and matching

Routes are matched by method and path. A request to a known path with an
unsupported method returns `405 Method Not Allowed`; an unknown path returns
`404 Not Found`.
