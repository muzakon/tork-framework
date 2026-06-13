# 11. WebSocket

A WebSocket endpoint upgrades an HTTP request to a long-lived, bidirectional
connection. Where [Server-Sent Events](10-server-sent-events.md) push one way,
a WebSocket lets both sides send messages until either closes. Use it for chat,
collaborative editing, live dashboards, and anything that needs the client to
talk back.

This chapter assumes you have read [Extractors and dependency injection](04-extractors-and-dependency-injection.md).

## A first socket

Declare an endpoint with `#[websocket]`. The handler takes a `WebSocket` (the
upgrade handle) and returns `tork::Result<()>`. Call `accept` to complete the
upgrade and obtain the live connection, then loop over incoming messages.

```rust
use tork::{websocket, WebSocket, WsMessage};

#[websocket("/ws")]
pub async fn echo(socket: WebSocket) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while let Some(message) = socket.recv().await? {
        match message {
            WsMessage::Text(text) => socket.send_text(text).await?,
            WsMessage::Binary(bytes) => socket.send_binary(bytes).await?,
            WsMessage::Close(_) => break,
            _ => {}                       // ping and pong are answered for you
        }
    }
    Ok(())
}
```

`recv` returns `Ok(None)` once the peer closes. It also returns `Ok(None)` when
the server begins a graceful shutdown: Tork first sends the client a `1001 Going
Away` close frame so the loop ends cleanly instead of the connection being dropped
mid-flight. `send`, `send_text`, and `send_binary` write messages back. The
WebSocket wire protocol is handled for you; you never see the framing.

## Typed messages

For structured messages, exchange JSON. `receive_json` reads the next text or
binary message and deserializes it; `send_json` serializes and sends.

```rust
#[api_model]
pub struct ChatIn { pub message: String }

#[api_model]
pub struct ChatOut { pub from: String, pub message: String }

#[websocket("/chat/{room_id}")]
pub async fn chat(socket: WebSocket, room_id: String, user: CurrentUser) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    while let Some(input) = socket.receive_json::<ChatIn>().await? {
        socket
            .send_json(&ChatOut { from: user.name.clone(), message: input.message })
            .await?;
    }
    Ok(())
}
```

A malformed JSON payload is a `400` error from `receive_json`.

## Validating messages

`receive_valid::<T>()` deserializes the next message and then runs its `garde`
validation, so each message is checked the way a request body is. An invalid
message is a `422` error listing the offending fields:

```rust
#[api_model]
pub struct ChatIn {
    #[field(min_length = 1, max_length = 500)]
    pub message: String,
}

while let Some(input) = socket.receive_valid::<ChatIn>().await? {
    // `input` is deserialized and validated.
}
```

## Limits and timeouts

Set limits for the whole app with `App::websocket_config`, or per route with the
`#[websocket(...)]` attributes (a route value overrides the app default):

```rust
use std::time::Duration;
use tork::WebSocketConfig;

App::new().websocket_config(
    WebSocketConfig::new()
        .max_message_size_kb(64)
        .idle_timeout_secs(120)
        .handshake_timeout(Duration::from_secs(10))
        .max_connections_per_ip(20),
);

#[websocket("/chat/{room}", max_message_size = "64KB", idle_timeout = "60s")]
pub async fn chat(socket: WebSocket /* ... */) -> tork::Result<()> { /* ... */ }
```

`max_message_size` and `max_frame_size` bound incoming data; `idle_timeout` closes
a connection that has been silent for too long (sizes accept `"64KB"`/`"1MB"`,
durations accept `"500ms"`/`"60s"`/`"2m"`).

Two limits guard against denial-of-service:

- `handshake_timeout` (default 10 seconds) bounds how long the upgrade may take, so
  a client that opens the upgrade and then stalls cannot hold a connection slot
  forever.
- `max_connections_per_ip` caps how many WebSockets one client IP may hold open at
  once; further connections from that IP are rejected with `429 Too Many Requests`
  before the upgrade. This is an application-level setting (`App::websocket_config`).

## Lifecycle hooks

Observe connections opening and closing with app-level hooks. `on_ws_connect`
fires once the socket opens; `on_ws_disconnect` fires when it closes, carrying how
long it was open and the close code:

```rust
App::new()
    .on_ws_connect(|info| async move { tracing::info!("ws open {}", info.path()) })
    .on_ws_disconnect(|info| async move {
        tracing::info!("ws closed after {:?} ({:?})", info.duration(), info.close_code());
    });
```

## Broadcasting with a hub

For fan-out (chat rooms, live feeds), `Hub` holds named `Room`s, each a broadcast
channel. A `Hub` is cheap to clone, so wrap it in a resource and inject it. A
typical room handler selects between the socket and the room's subscription:

```rust
#[websocket("/chat/{room}", incoming = ChatIn, outgoing = ChatMessage)]
pub async fn chat(socket: WebSocket, room: String, hub: ChatHub) -> tork::Result<()> {
    let mut socket = socket.accept().await?;
    let room = hub.0.room(room);
    let mut receiver = room.subscribe();
    loop {
        tokio::select! {
            incoming = socket.receive_valid::<ChatIn>() => match incoming? {
                Some(input) => { room.broadcast(ChatMessage { from: "guest".into(), text: input.message }); }
                None => break,
            },
            outgoing = receiver.recv() => match outgoing {
                Ok(message) => socket.send_json(&message).await?,
                Err(_) => break,
            },
        }
    }
    Ok(())
}
```

`room.broadcast(message)` sends to every current subscriber and returns how many
it reached. (Because `Hub` is a foreign type, wrap it in a local newtype to inject
it as a resource: `#[derive(Clone)] pub struct ChatHub(pub Hub<ChatMessage>);`.)

A `Hub` is safe for dynamically named rooms (per user or per session): creating a
new room first evicts any room that has become dead — no subscribers and no
outstanding `Room` handles — so the registry does not grow without bound.

## Documenting channels with AsyncAPI

WebSocket message contracts do not fit OpenAPI, so declare them with `incoming` and
`outgoing` and serve an AsyncAPI document:

```rust
use tork::AsyncApi;

#[websocket("/chat/{room}", incoming = ChatIn, outgoing = ChatMessage)]
pub async fn chat(/* ... */) -> tork::Result<()> { /* ... */ }

App::new().asyncapi(AsyncApi::new().title("Realtime API").version("1.0.0").json("/asyncapi.json"));
```

Each WebSocket route becomes a channel with `receive` (incoming) and `send`
(outgoing) operations, and each SSE route a `send` channel, with the message
payloads under `components.schemas`.

## Dependencies resolve before the upgrade

Every parameter other than the `WebSocket` is a path parameter or a dependency,
resolved exactly as in a normal route: path captures, extractors, services, and
guards such as `CurrentUser` all work. Crucially, they resolve **before** the
upgrade. If any of them fails, the request is rejected with its HTTP status and no
socket is ever opened:

```rust
#[websocket("/admin/ws")]
pub async fn admin_socket(socket: WebSocket, admin: AdminUser) -> tork::Result<()> {
    // If `AdminUser` rejects (say 403), the client sees the HTTP error and the
    // handshake never completes.
    let mut socket = socket.accept().await?;
    // ...
    Ok(())
}
```

This is the right place to authenticate: a browser cannot read a custom close
code, but it can see the failed upgrade response.

## Closing

Close the connection cleanly with a status code and reason:

```rust
use tork::WsCloseCode;

socket.close(WsCloseCode::NormalClosure, "done").await?;
```

`WsCloseCode` covers the common codes (`NormalClosure`, `GoingAway`,
`PolicyViolation`, `MessageTooBig`, `InternalError`, and others), with
`as_u16` / `from_u16` for the raw value. `WsError` carries a close code and a
message; before the upgrade it converts into an HTTP error (so a guard can reject
with, for example, a policy violation).

Once the connection is accepted the HTTP status is already sent, so it can no
longer change. If the handler returns `Err` after that point, the error is logged
and the connection drops; for a clean shutdown, call `close` yourself.

## Messages

`WsMessage` is the message type both directions use:

- `Text(String)` and `Binary(Vec<u8>)`: application data.
- `Ping(Vec<u8>)` and `Pong(Vec<u8>)`: control frames. The protocol layer answers
  pings automatically; they are surfaced only if you want to observe them.
- `Close(Option<WsClose>)`: the peer is closing, optionally with a code and reason.

## Middleware

A WebSocket starts as a normal `GET` request, so middleware runs during the
handshake: request id, tracing, CORS, and authentication middleware all apply to
the upgrade. Once the connection is upgraded, the request and response pipeline no
longer applies; messages flow directly over the socket.
