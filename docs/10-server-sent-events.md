# 10. Server-Sent Events

Server-Sent Events (SSE) is a one-way streaming protocol over plain HTTP: the
server holds the response open and pushes `text/event-stream` messages to the
client as they happen. It fits live feeds, progress updates, notifications, and
token-by-token model output. A handler returns an `Sse<T>` built from a stream,
and Tork takes care of the headers, the wire encoding, and keeping the connection
alive.

This chapter assumes you have read [Responses and errors](06-responses-and-errors.md).

## A first stream

Declare an endpoint with `#[sse]`. The handler returns `tork::Result<Sse<T>>`,
where `T` is the data type each event carries. `Sse::new` accepts any stream of
`Result<T>`.

```rust
use tork::{sse, Sse};

#[sse("/items/stream", event = "item_update", response_model = ItemOut)]
pub async fn stream_items(service: ItemService) -> tork::Result<Sse<ItemOut>> {
    let items = service.item_stream();          // any Stream<Item = tork::Result<ItemOut>>
    Ok(Sse::new(items))
}
```

The stream can come from anywhere: a channel, a database cursor, or the
`async-stream` crate's `try_stream!` for a `yield`-style body. Tork does not pick
one for you.

The response is set up automatically:

```
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
X-Accel-Buffering: no
```

and each item is encoded as one event, with the name from `event = ...`:

```
event: item_update
data: {"id":1,"name":"Keyboard"}

event: item_update
data: {"id":2,"name":"Mouse"}
```

## Per-event control

For full control over an event (its name, id, retry hint, or comment), stream
`SseEvent<T>` values and use `Sse::events`:

```rust
use tork::{sse, Sse, SseEvent};

#[sse("/items/stream")]
pub async fn stream_items(service: ItemService) -> tork::Result<Sse<ItemOut>> {
    let events = service.items().map(|item| {
        let item = item?;
        Ok(SseEvent::new(ItemOut::from(&item))
            .event("item_update")
            .id(item.id)            // sets the SSE event id
            .retry_ms(5000))        // client reconnection delay
    });
    Ok(Sse::events(events))
}
```

`SseEvent::new(value)` carries a typed payload (serialized to JSON);
`SseEvent::raw("[DONE]")` carries a pre-formatted string instead. The two are
mutually exclusive: an event has either typed data or raw data, never both.
`.comment("...")` adds a comment line, which clients ignore.

## Configuring the stream

`Sse` has builder methods for the response behavior:

```rust
use std::time::Duration;

Ok(Sse::new(items)
    .event("item_update")                       // default event name for data items
    .heartbeat(Duration::from_secs(15))         // keep-alive comment (default 15s)
    .max_event_size(64 * 1024)                   // drop oversized events
    .client_timeout(Duration::from_secs(60))     // end the stream after this long
    .done_event("[DONE]"))                        // final data event when the source ends
```

The defaults are sensible for production: a 15 second heartbeat (call
`.no_heartbeat()` to turn it off), `Cache-Control: no-cache`, and
`X-Accel-Buffering: no` so reverse proxies do not buffer the stream. The
compression middleware automatically skips `text/event-stream` responses.

## Limiting concurrent streams

Each open SSE stream holds a pinned stream and its timers for its whole lifetime,
which can be hours, so an unbounded number of them can exhaust memory. Cap the
concurrent count on the app:

```rust
App::new().max_sse_connections(10_000)
```

Once the cap is reached, further SSE requests are rejected with
`503 Service Unavailable` until an existing stream ends and frees a slot. With no
cap set, SSE streams are unbounded.

## Resuming with Last-Event-ID

When a browser reconnects after a drop, it sends the id of the last event it saw
in the `Last-Event-ID` header. The `LastEventId` extractor reads it, so you can
resume from where the client left off:

```rust
use tork::{sse, Sse, LastEventId};

#[sse("/notifications")]
pub async fn notifications(
    last: LastEventId,
    service: NotificationService,
) -> tork::Result<Sse<NotificationOut>> {
    let after = last.as_str().and_then(|id| id.parse::<i64>().ok()).unwrap_or(0);
    Ok(Sse::new(service.after(after)))
}
```

For this to work, set an id on each event (`SseEvent::new(..).id(..)`).

When the id is a known type, `SseResume<T>` parses it for you (a missing or
invalid value yields `None`):

```rust
use tork::{sse, Sse, SseResume};

#[sse("/notifications")]
pub async fn notifications(
    resume: SseResume<i64>,
    service: NotificationService,
) -> tork::Result<Sse<NotificationOut>> {
    let after = resume.last_id().copied().unwrap_or(0);
    Ok(Sse::new(service.after(after)))
}
```

## Streaming over POST

SSE is not limited to `GET`. Use `#[post_sse]` (or `#[sse(method = POST, ...)]`)
to stream a response to a request that carries a body, for example streaming model
tokens for a prompt:

```rust
use tork::{post_sse, Sse, Valid};

#[post_sse("/chat/stream", event = "token")]
pub async fn stream_chat(
    body: Valid<ChatPrompt>,
    service: ChatService,
) -> tork::Result<Sse<TokenOut>> {
    Ok(Sse::new(service.stream_tokens(body.into_inner())))
}
```

Dependency injection, validation, and the other extractors work exactly as they
do for a normal route.

## OpenAPI

An SSE route still appears in the OpenAPI document. Its `200` response is
documented as `text/event-stream` with the `response_model` schema and a
"Server-Sent Events stream" description, so consumers know the body is a stream of
JSON-encoded values rather than a single object.

## Errors

A handler error before the response is returned (for example a failed service
call while building the `Sse`) is a normal error with the proper status code. Once
the stream has started, the status is already `200`; an error from the stream
itself is logged and ends the stream, since the headers can no longer change.
