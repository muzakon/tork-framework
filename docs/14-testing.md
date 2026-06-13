# 14. Testing

Tork ships an in-process test client. It builds your application once (running the
lifespan), then sends requests straight through the request pipeline without a
network socket: no port to bind, no race conditions, fast tests. The same client
opens WebSockets and reads Server-Sent Events, overrides resources and
dependencies, and runs the lifespan shutdown when finished.

This chapter assumes you have read [Extractors and dependency injection](04-extractors-and-dependency-injection.md).

## A first test

Build a `TestApp` with `App::build_test`, wrap it in a `TestClient`, and send
requests. Tests are async, so use `#[tokio::test]`.

```rust
use serde_json::json;
use tork::testing::TestClient;
use tork::{get, App, Router};

#[get("/")]
async fn read_main() -> tork::Result<serde_json::Value> {
    Ok(json!({ "msg": "Hello World" }))
}

#[tokio::test]
async fn reads_main() -> tork::Result<()> {
    let app = App::new().include(read_main).build_test().await?;
    let client = TestClient::new(app).await?;

    let response = client.get("/").send().await?;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await?,
        json!({ "msg": "Hello World" })
    );
    Ok(())
}
```

`App::include(handler)` registers a single handler by its generated route factory;
`include_router` still works for mounting a whole router.

## Requests and responses

The verb methods (`get`, `post`, `put`, `patch`, `delete`) return a request
builder. Set a JSON, form, or multipart body, add headers and query parameters,
then `send`.

```rust
// JSON body.
let response = client
    .post("/items")
    .header("x-tenant", "acme")
    .query("dry_run", "true")
    .json(&json!({ "id": "foo", "name": "Foo" }))
    .send()
    .await?;

// Urlencoded form.
let response = client
    .post("/login")
    .form(&json!({ "username": "ada", "password": "secret" }))
    .send()
    .await?;
```

`TestResponse` reads the captured response:

```rust
response.status();                       // u16
response.status_code();                  // http::StatusCode
response.headers();                      // &HeaderMap
response.text()?;                        // String
response.bytes();                        // Bytes
response.json::<ItemOut>().await?;       // deserialize JSON
```

### File uploads

`multipart()` builds a `multipart/form-data` body with text fields and files,
matching the [forms and file uploads](12-forms-and-file-uploads.md) parsers.

```rust
let response = client
    .post("/files")
    .multipart()
    .text("token", "secret-token")
    .file_bytes("file", "hello.txt", "text/plain", b"hello world".to_vec())
    .send()
    .await?;
```

## Cookies and default headers

A `TestClient` keeps a cookie jar: every `Set-Cookie` in a response is stored and
replayed on later requests, so a login flow carries its session automatically. Use
the builder to set headers and cookies sent with every request.

```rust
let client = TestClient::builder(app)
    .default_header("X-Token", "cone-of-silence")
    .cookie("session", "seed-value")
    .build()
    .await?;
```

In-process tests reject security-sensitive routing/proxy headers (`Host`,
`Forwarded`, `X-Forwarded-*`) by default so a unit-style test cannot silently
spoof proxy state. When you need that behavior, either opt in explicitly:

```rust
let response = client
    .get("/")
    .unsafe_header("host", "example.com")
    .unsafe_header("x-forwarded-proto", "https")
    .send()
    .await?;
```

or run the app over a real loopback socket with
`TestClient::serve(app).bind_random_port().await?`.

## WebSockets

`websocket(path).connect()` opens a connection over an in-memory duplex (no
socket). It returns an error if the handshake or a dependency is rejected before
the upgrade.

```rust
let mut ws = client.websocket("/ws").connect().await?;
ws.send_text("hello").await?;
assert_eq!(ws.receive_text().await?, "hello");

ws.send_json(&json!({ "message": "hi" })).await?;
let reply = ws.receive_json::<ChatOut>().await?;

ws.close().await?;
```

The builder accepts headers, query parameters, and subprotocols:

```rust
let mut ws = client
    .websocket("/rooms/general")
    .query("token", "abc")
    .subprotocol("chat.v1")
    .connect()
    .await?;
```

## Server-Sent Events

`sse()` reads a streaming response event by event. Heartbeat comments are skipped,
so each returned event carries a name, data, or id.

```rust
let mut stream = client.get("/events").sse().await?;

let event = stream.next_event().await?.expect("an event");
assert_eq!(event.event(), Some("tick"));
assert_eq!(event.json::<EventOut>()?.n, 1);

// Or bound the wait:
let event = stream.next_event_timeout(Duration::from_secs(1)).await?;
```

## Overriding resources and dependencies

Tests should not touch a production database or read `.env`. The builder overrides
both resources and injected dependencies. A resource override is applied after the
lifespan runs, so it wins over the value the lifespan registered.

```rust
let client = TestClient::builder(app)
    .resource(Arc::new(Config { /* test values */ }))
    .resource(TestDb::new())
    .override_dependency::<EmailService>(EmailService::mock())
    .build()
    .await?;
```

Because Tork injects concrete types, an override supplies an instance of the same
type the handler asks for (not an unrelated mock type). `override_dependency` takes
a value cloned per request; `override_dependency_with` takes a factory for types
that are not `Clone`. The override bypasses the service's normal field resolution,
so its dependencies need not be registered in the test.

## Lifespan and shutdown

`build_test` runs the lifespan startup, so resources a lifespan registers are
available in tests. Rust has no async `Drop`, so call `shutdown` to run the
lifespan teardown when the test is done.

```rust
let app = App::new().lifespan::<AppState>().include_router(api::router());
let client = TestClient::new(app.build_test().await?).await?;

// ... requests ...

client.shutdown().await?;
```

## End-to-end over a real port

The in-process client is the default and covers most needs. For a true end-to-end
test (real sockets, the full HTTP stack), bind the app to a loopback port:

```rust
let client = TestClient::serve(app).bind_random_port().await?;
let response = client.get("/health").send().await?;
let mut ws = client.websocket("/ws").connect().await?;
client.shutdown().await?;
```

The request, response, and WebSocket API is identical, so a test reads the same
whether it runs in process or over a socket.
