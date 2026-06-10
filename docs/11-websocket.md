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

`recv` returns `Ok(None)` once the peer closes. `send`, `send_text`, and
`send_binary` write messages back. The WebSocket wire protocol is handled for
you; you never see the framing.

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
