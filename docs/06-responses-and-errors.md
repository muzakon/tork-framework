# 6. Responses and errors

## Returning a response

A handler returns `tork::Result<T>`. On success, `T` is serialized to JSON with
the route's status code (default `200`, or whatever `status_code` you set):

```rust
#[post("/", response_model = OrderOut, status_code = 201)]
pub async fn create_order(/* ... */) -> tork::Result<OrderOut> {
    Ok(OrderOut { id: 1, user_id: 1, total_cents: 999 })
}
```

The success type must implement JSON Schema when the OpenAPI feature is on, which
`#[api_model]` provides.

## Response models

When `response_model` differs from the handler's return type, the returned value
is converted into the response model before serialization. Provide a `From`
implementation:

```rust
#[get("/{user_id}", response_model = UserOut)]
pub async fn get_user(user_id: i64, service: UserService) -> tork::Result<UserEntity> {
    service.get_user(user_id).await
}

impl From<UserEntity> for UserOut {
    fn from(entity: UserEntity) -> Self {
        Self { id: entity.id, email: entity.email }
    }
}
```

The handler returns the internal `UserEntity`; the client receives `UserOut`, and
the OpenAPI schema is `UserOut`.

## The error type

`tork::Error` carries a category, a client-facing message, an optional cause, and
optional field details. Construct one with a category helper:

```rust
use tork::Error;

Error::bad_request("invalid query");
Error::unauthorized("missing token");
Error::forbidden("access denied");
Error::not_found("user not found");
Error::conflict("already exists");
Error::payload_too_large("request body too large");
Error::unprocessable("could not process the request");
Error::too_many_requests("slow down");
Error::internal("unexpected failure");
Error::service_unavailable("try again later");
Error::gateway_timeout("upstream timed out");
```

Handlers return errors with `?` or `Err(...)`. They render to a response
automatically.

## The error response shape

Every error renders to a flat JSON body:

```json
{
  "status": 404,
  "code": "NOT_FOUND",
  "title": "Not Found",
  "message": "user not found",
  "traceId": "req-d01618f2-f39c-439e-93fd-21db43ec1cbd",
  "timestamp": "2026-06-10T11:08:07Z"
}
```

- `status` is the numeric HTTP status; `title` is its reason phrase.
- `code` is a stable upper snake-case identifier. By default it mirrors the
  status (`NOT_FOUND`, `INTERNAL_SERVER_ERROR`), and validation uses
  `VALIDATION_ERROR`. Override it per error with `Error::with_code("USER_NOT_FOUND")`.
- `details` is present only for field-level errors (see chapter 5). Add your own
  with `Error::with_details(...)`.
- `traceId` is generated per error response and is also logged for server errors,
  so a client-reported id can be matched against the logs.
- `timestamp` is the time of the response in RFC 3339 UTC.

## Server errors are redacted

For any `5xx`, the body never exposes the internal message. Only a generic message
is sent, while the real message and any cause are logged with the trace id:

```rust
// The client sees "Internal server error"; the log records the real detail.
Err(Error::internal("database password is wrong").with_source(db_error))
```

This keeps internal failures from leaking to clients while preserving the
information you need to debug them.

## Streaming responses

Most responses are fully buffered. For frame-at-a-time output, `RespBody::stream`
returns a streaming body (Server-Sent Events use this). A streaming body has no
inherent size limit, so for a generated download you can cap the total bytes with
`RespBody::stream_capped(body, max_bytes)`: the response errors once it exceeds the
limit, so a runaway producer cannot stream without end. Leave SSE on the uncapped
`stream`, since event streams are intentionally open-ended.
