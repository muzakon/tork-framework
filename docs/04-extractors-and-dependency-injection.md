# 4. Extractors and dependency injection

Every handler parameter that is not a path parameter is resolved from the request
through the `FromRequest` trait. Tork resolves them in declaration order before
calling the handler.

## Built-in extractors

- `State<S>`: a clone of a registered application state value.
- `BearerToken`: the token from an `Authorization: Bearer ...` header.
- `Json<T>`: the request body parsed as JSON into `T`.
- `Valid<T>`: the request body parsed as JSON and validated (see chapter 5).

At most one parameter may consume the request body.

## Application state

State is registered on the `App` and read with `State<S>`. The value is cloned per
request, so it should be cheap to clone (hold pools and handles behind `Arc`):

```rust
use std::collections::HashMap;
use std::sync::Arc;

use tork::{Error, Result, State, api_router, get};

use crate::models::user::UserOut;

#[derive(Clone)]
pub struct AppState {
    users: Arc<HashMap<i64, UserOut>>,
}

impl AppState {
    pub fn get_user(&self, id: i64) -> Result<UserOut> {
        self.users
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::not_found("user not found"))
    }
}

#[api_router(prefix = "/users", tags = ["users"])]
pub mod users_router {
    use super::*;

    #[get("/{user_id}", response_model = UserOut, summary = "Get user by id")]
    pub async fn get_user(user_id: i64, state: State<AppState>) -> Result<UserOut> {
        state.0.get_user(user_id)
    }
}
```

Register the value when building the app: `App::new().state(AppState::boot().await?)`.

## Custom dependencies

A dependency is any type that knows how to build itself from the request. Annotate
an `impl` block that has an async `resolve` function with `#[tork::dependency]`:

```rust
use tork::{State, dependency};

use crate::core::app_state::AppState;
use crate::models::user::UserOut;

#[derive(Clone)]
pub struct UserRepository {
    state: AppState,
}

#[tork::dependency]
impl UserRepository {
    pub async fn resolve(state: State<AppState>) -> tork::Result<Self> {
        Ok(Self { state: state.0 })
    }
}

impl UserRepository {
    pub async fn find_by_id(&self, id: i64) -> tork::Result<UserOut> {
        self.state.get_user(id)
    }
}
```

`resolve` may take other dependencies as parameters. They are resolved first, then
`resolve` runs. This makes dependency graphs compose:

```rust
pub struct UserService {
    users: UserRepository,
}

#[tork::dependency]
impl UserService {
    pub async fn resolve(users: UserRepository) -> tork::Result<Self> {
        Ok(Self { users })
    }
}
```

Now a handler can simply ask for the service:

```rust
#[get("/{user_id}", response_model = UserOut)]
pub async fn get_user(user_id: i64, service: UserService) -> tork::Result<UserOut> {
    service.get_user(user_id).await
}
```

Resolving `UserService` builds a `UserRepository`, which reads `State<AppState>`.

## Authentication example

`BearerToken` and a dependency combine into a current-user guard:

```rust
use tork::{BearerToken, Error, dependency};

use crate::repositories::user_repository::UserRepository;

#[derive(Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub email: String,
}

#[tork::dependency]
impl CurrentUser {
    pub async fn resolve(token: BearerToken, users: UserRepository) -> tork::Result<Self> {
        let user_id = users.authenticate(token.token())?;
        let user = users.find_by_id(user_id).await?;
        Ok(CurrentUser { id: user.id, email: user.email })
    }
}

impl CurrentUser {
    pub fn ensure_can_access_user(&self, user_id: i64) -> tork::Result<()> {
        if self.id != user_id {
            return Err(Error::forbidden("access denied"));
        }
        Ok(())
    }
}
```

A handler that lists a user's orders can now require authentication and authorize
the path:

```rust
#[get("/", response_model = Vec<OrderOut>, summary = "List orders for a user")]
pub async fn list_user_orders(
    user_id: i64,
    current_user: CurrentUser,
    service: OrderService,
) -> tork::Result<Vec<OrderOut>> {
    current_user.ensure_can_access_user(user_id)?;
    service.list_orders_for_user(user_id).await
}
```

A missing or malformed token yields `401`, a forbidden access yields `403`.
