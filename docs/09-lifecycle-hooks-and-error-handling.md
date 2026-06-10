# 9. Lifecycle hooks and error handling

Hooks let you observe the request lifecycle without changing it. Exception
handlers let you map a typed error into a response. Together they cover logging,
metrics, tracing, custom error pages, and a safety net for panics.

This chapter assumes you have read [Responses and errors](06-responses-and-errors.md)
and [Middleware](08-middleware.md). Middleware sits in the request path and can
change the request and response. Hooks sit beside it and only watch.

## Where each piece runs

```
on_request                         (every request, before middleware)
  -> middleware chain
       -> handler / extractors / validation   (panic boundary, if enabled)
            on_error or on_validation_error    (when an error is produced)
            exception_handler                  (when a typed error matches)
            response
       <- middleware unwind
  on_response                        (every response, after middleware)
```

A handler error is rendered inside the chain, so response middleware (such as
compression or CORS headers) still decorates error responses.

## Observability hooks

The four observability hooks are observe-only: they receive an event, return
nothing, and cannot change the response. They run in registration order, inline
with the request, so keep them quick.

```rust
use tork::{App, RequestEvent, ResponseEvent, ErrorEvent, ValidationErrorEvent};

App::new()
    .on_request(|event: RequestEvent| async move {
        println!("--> {} {}", event.method(), event.path());
    })
    .on_response(|event: ResponseEvent| async move {
        println!(
            "<-- {} {} {} ({:.1?})",
            event.method(),
            event.path(),
            event.status(),
            event.elapsed()
        );
    })
    .on_error(|event: ErrorEvent| async move {
        // Fires for every non-validation error.
        eprintln!("error {} on {}: {}", event.code(), event.path(), event.message());
    })
    .on_validation_error(|event: ValidationErrorEvent| async move {
        // Fires only for a 422 validation failure.
        eprintln!("{} field(s) failed validation on {}", event.details().len(), event.path());
    })
    .include_router(users::router())
    .serve("0.0.0.0:8000")
    .await
```

Every event exposes the request method, path, the matched route pattern, and the
request id:

- `method()`, `path()`: the HTTP method and the concrete request path.
- `route()`: the matched route pattern (for example `/users/{id}`). It is present
  for `on_error`, `on_validation_error`, and `on_panic`, which fire once routing
  has run; it is `None` for `on_request` and `on_response`.
- `request_id()`: the `x-request-id` value when present. Register the
  [`RequestId`](08-middleware.md) middleware so every request carries one.

Beyond the shared accessors:

- `ResponseEvent` adds `status()` and `elapsed()`.
- `ErrorEvent` adds `status()`, `code()`, and `message()`.
- `ValidationErrorEvent` adds `details()`, the list of field-level failures.

A validation failure (`422`) fires only `on_validation_error`; every other error
fires `on_error`. The two never both fire for the same error.

Registering several hooks of one kind is fine; they run in order:

```rust
App::new()
    .on_request(record_metric)
    .on_request(start_trace_span);   // both run, in this order
```

When no hook is registered, the request path does no extra work.

## Scoping hooks to routers and routes

The four observability hooks can also be attached to a single router or route, so
a concern targets part of the API instead of every request. Scoped hooks run in
addition to the app-global ones. (`exception_handler` and `catch_panics` stay
app-level.)

Attach to a router with the same builder methods; every route in the router (and
any nested router) inherits them:

```rust
App::new()
    .include_router(
        admin::router()
            .on_request(audit_request)   // every /admin route
            .on_error(alert_on_error),
    )
    .include_router(public::router())     // unaffected
    .serve("0.0.0.0:8000")
    .await
```

Attach to a single route by naming a function in the route macro. An attribute
cannot hold a closure, so the value is a function path; each kind is repeatable:

```rust
#[api_router(prefix = "/admin", tags = ["admin"])]
pub mod admin_router {
    use super::*;

    #[get("/users", on_request = audit_request, on_response = record_latency)]
    pub async fn list_users(repo: UserRepository) -> tork::Result<Vec<UserOut>> { /* ... */ }
}

// The named hook lives next to the routes (in the parent module):
async fn audit_request(event: RequestEvent) {
    println!("audit: {} {}", event.method(), event.path());
}
```

A hand-built route uses the same builders:

```rust
Route::new(Method::GET, "/health", handler).on_response(record_latency);
```

Scoped hooks fire after routing (once the route is known), so a route-scoped
`on_request` does not run when a middleware short-circuits or no route matches.
They compose with the app-global hooks in this order:

```
app on_request                         (before middleware)
  -> middleware
       scoped on_request               (outer router -> inner router -> route)
       handler
       scoped on_error / on_validation_error  (after the app-global ones)
       scoped on_response              (route -> inner -> outer router)
  <- middleware unwind
app on_response                        (after middleware)
```

## Typed errors and exception handlers

A handler often calls into code that fails with its own error type. Derive
`AppError` on that type to convert it into a framework error through `?`, and
register an `exception_handler` to map it to a response.

```rust
use tork::{ErrorContext, IntoResponse, Response};

#[derive(Debug, tork::AppError)]
#[status(503)]                       // default status when no handler is registered
pub enum RepoError {
    Unavailable,
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::Unavailable => f.write_str("the data store is unavailable"),
        }
    }
}

impl std::error::Error for RepoError {}
```

`#[status(...)]` accepts a status code (for example `503`) or an `ErrorKind`
variant name (for example `ServiceUnavailable`). It defaults to `500`. The type
must implement `Display` and `std::error::Error`; the derive only adds the
conversion.

Now a repository can use `?` directly:

```rust
pub async fn find(&self, id: i64) -> tork::Result<UserOut> {
    let row = self.fetch(id).await?;   // RepoError -> tork::Error
    Ok(row.into())
}
```

Register a handler to control the response. It receives the recovered typed value
and a context with the request metadata:

```rust
async fn on_repo_error(error: RepoError, ctx: ErrorContext) -> Response {
    eprintln!("repo error on {}: {error}", ctx.path());
    tork::Error::service_unavailable("the data store is temporarily unavailable")
        .into_response()
}

App::new()
    .exception_handler::<RepoError, _, _>(on_repo_error)
    .include_router(users::router())
    .serve("0.0.0.0:8000")
    .await
```

The handler matches the error's directly attached source. With no handler
registered, the error renders with its declared status using the standard
problem-details body. As always, the body of a `5xx` response is redacted to a
generic message, while the detail is logged server-side.

## Catching panics

By default a panic in a handler propagates and drops that connection. Enable the
panic boundary to turn it into a `500` instead, and observe it with `on_panic`:

```rust
use tork::PanicEvent;

App::new()
    .catch_panics()
    .on_panic(|event: PanicEvent| async move {
        eprintln!("panic on {}: {}", event.path(), event.message());
    })
    .include_router(users::router())
    .serve("0.0.0.0:8000")
    .await
```

A caught panic returns the generic `500` response and fires `on_panic` only; it
does not run `on_error` or any exception handler. The boundary is opt-in and has
no effect when the binary is built with `panic = "abort"`.

## Hooks and the application lifecycle

These request hooks are independent of the application lifecycle. You can combine
`catch_panics`, the request hooks, and an exception handler with a lifespan (or
with startup and shutdown hooks) freely.
