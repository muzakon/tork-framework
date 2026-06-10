# 8. Middleware

Middleware wraps request handling. Each layer receives the request and a `Next`
handle, may inspect or modify the request, calls `next` to run the rest of the
chain, and may inspect or modify the response. Layers run in registration order,
outermost first.

## Registering middleware

```rust
use tork::App;
use tork::middleware::{Compression, Cors, RequestId, Trace};

App::new()
    .middleware(RequestId::new())
    .middleware(Trace::new())
    .middleware(
        Cors::new()
            .allow_origin("https://app.example.com")
            .allow_methods(["GET", "POST", "PUT", "DELETE"])
            .allow_headers(["Authorization", "Content-Type"])
            .expose_headers(["X-Request-Id"]),
    )
    .middleware(Compression::new().gzip().minimum_size(1000))
    .include_router(users::router())
    .serve("0.0.0.0:8000")
    .await
```

## Built-in middleware

All of these live under `tork::middleware`:

- `RequestId::new()`: reuses an incoming `x-request-id` or generates `req-<uuid>`,
  and sets it on the request and the response. `header_name("...")` changes the
  header.
- `Trace::new()`: logs each request's method, path, status, and elapsed time.
- `Timeout::seconds(30)`: fails a request with `504` if it runs past the deadline.
- `BodyLimit::mb(10)`: rejects a request with `413` when its declared
  `Content-Length` exceeds the limit. Also `::kb` and `::bytes`.
- `TrustedHost::new(["example.com", "*.example.com"])`: rejects with `400` when the
  `Host` header is not in the allow-list. `*.` matches any subdomain.
- `HttpsRedirect::new()`: redirects plain HTTP to HTTPS with `308`. The scheme is
  read from `X-Forwarded-Proto` when present.
- `ProxyHeaders::new()`: rewrites `Host` from `X-Forwarded-Host`, so host-based
  layers see the client-facing host. Register it before `TrustedHost` and
  `HttpsRedirect`.
- `Cors::new()`: answers preflight `OPTIONS` requests and adds CORS headers to
  actual responses. Builder: `allow_origin`, `allow_methods`, `allow_headers`,
  `expose_headers`, `allow_credentials`, `max_age`. Use `allow_origin("*")` to
  allow any origin.
- `Compression::new().gzip().minimum_size(1000)`: gzip-compresses responses when
  the client accepts gzip and the body meets the minimum size.

## Custom middleware

Write an `async fn(request, next) -> Result<Response>` and annotate it with
`#[middleware]`. The header types are re-exported from `tork`, so no extra
dependency is needed:

```rust
use tork::{middleware, HeaderName, HeaderValue, Next, Request, Response, Result};

#[middleware]
pub async fn add_process_time(req: Request, next: Next) -> Result<Response> {
    let start = std::time::Instant::now();
    let mut response = next.run(req).await?;

    let elapsed = start.elapsed().as_secs_f64();
    if let Ok(value) = HeaderValue::from_str(&format!("{elapsed:.6}")) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-process-time"), value);
    }
    Ok(response)
}

// App::new().middleware(add_process_time)
```

`next.run(req)` runs the rest of the chain and returns its response. Returning
`Err(...)` short-circuits the chain; the error renders as a response. Returning a
response without calling `next` short-circuits too (this is how `Cors` answers a
preflight).

## Single-registration middleware

Some middleware should be registered only once. Registering it twice is almost
always a bug, so these layers reject duplicates at startup:

```rust
App::new()
    .middleware(Cors::new().allow_origin("*"))
    .middleware(Cors::new().allow_origin("https://app.com"))
    .serve("0.0.0.0:8000")
    .await
```

This fails when the application is built, before it serves:

```
Duplicate middleware detected: Cors
Cors middleware can only be registered once per scope.
Already registered at app level.
```

Each middleware reports a duplicate policy. The policies are:

- `Allow`: keep every registration (the default for custom middleware).
- `Warn`: keep every registration and log a warning.
- `Reject`: fail the build with the message above.
- `Replace`: keep only the most recent registration.

`RequestId`, `Trace`, `Cors`, `Compression`, `TrustedHost`, `HttpsRedirect`,
`BodyLimit`, `ProxyHeaders`, and `Timeout` use `Reject`.
