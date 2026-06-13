//! Server-Sent Events: a typed, streaming `text/event-stream` response.
//!
//! A handler returns [`Sse<T>`], built from any stream of items (each item
//! becomes a `data:` event) or of [`SseEvent<T>`] (full per-event control). The
//! response sets the standard SSE headers, encodes each event to the wire format,
//! and can emit periodic heartbeats so idle connections stay open through proxies.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::{BoxStream, StreamExt};
use http::header::{HeaderName, HeaderValue, CACHE_CONTROL, CONTENT_TYPE};
use http_body::{Body, Frame, SizeHint};
use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{interval_at, sleep, Instant, Interval, Sleep};

use crate::body::{BoxError, RespBody};
use crate::constants::TEXT_EVENT_STREAM;
use crate::error::{Error, Result};
use crate::extract::RequestContext;
use crate::response::{IntoResponse, Response};

/// Default heartbeat interval, sent as an SSE comment to keep the stream alive.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(15);
/// Default cap on a single encoded SSE event, applied when the handler does not
/// set one. Bounds per-event memory (an event is cloned per subscriber when fanned
/// out through a broadcast hub); override with [`Sse::max_event_size`].
const DEFAULT_MAX_EVENT_SIZE: usize = 256 * 1024;
/// The heartbeat payload (an SSE comment line).
const HEARTBEAT_FRAME: &[u8] = b": ping\n\n";
/// A pre-wrapped `Bytes` view of the heartbeat frame, allocated once instead of
/// on every heartbeat tick.
static HEARTBEAT_BYTES: LazyLock<Bytes> = LazyLock::new(|| Bytes::from_static(HEARTBEAT_FRAME));
/// Header that tells reverse proxies not to buffer the response.
const X_ACCEL_BUFFERING: &str = "x-accel-buffering";

/// A single Server-Sent Event with optional metadata.
///
/// Construct it with [`SseEvent::new`] (a typed `data` payload) or
/// [`SseEvent::raw`] (a pre-formatted string). The two are mutually exclusive by
/// construction: there is no setter to add the other afterwards.
pub struct SseEvent<T> {
    data: Option<T>,
    raw: Option<String>,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    comment: Option<String>,
}

impl<T> SseEvent<T> {
    /// Creates an event whose `data` is a typed value, serialized to JSON.
    pub fn new(data: T) -> Self {
        Self {
            data: Some(data),
            raw: None,
            event: None,
            id: None,
            retry: None,
            comment: None,
        }
    }

    /// Creates an event whose `data` is a pre-formatted string (no serialization).
    pub fn raw(raw: impl Into<String>) -> Self {
        Self {
            data: None,
            raw: Some(raw.into()),
            event: None,
            id: None,
            retry: None,
            comment: None,
        }
    }

    /// Sets the event name (`event:` line).
    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    /// Sets the event id (`id:` line).
    pub fn id(mut self, id: impl ToString) -> Self {
        self.id = Some(id.to_string());
        self
    }

    /// Sets the reconnection time in milliseconds (`retry:` line).
    pub fn retry_ms(mut self, ms: u64) -> Self {
        self.retry = Some(ms);
        self
    }

    /// Sets a comment line (ignored by clients, useful for diagnostics).
    pub fn comment(mut self, text: impl Into<String>) -> Self {
        self.comment = Some(text.into());
        self
    }
}

impl<T: Serialize> SseEvent<T> {
    /// Serializes the typed data (if any) into the wire-ready [`RawEvent`].
    fn into_raw(self) -> Result<RawEvent> {
        let data = match (self.data, self.raw) {
            (Some(data), _) => Some(serde_json::to_string(&data).map_err(|error| {
                Error::internal(format!("failed to serialize SSE data: {error}"))
            })?),
            (None, Some(raw)) => Some(raw),
            (None, None) => None,
        };
        Ok(RawEvent {
            data,
            event: self.event,
            id: self.id,
            retry: self.retry,
            comment: self.comment,
        })
    }
}

/// An event whose data is already a string, ready to encode to the wire format.
struct RawEvent {
    data: Option<String>,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    comment: Option<String>,
}

/// Encodes an event to the SSE wire format, falling back to `default_event`.
///
/// Multi-line `data` and comments are split into one line each, as the protocol
/// requires, and the event is terminated by a blank line.
fn encode_event(event: &RawEvent, default_event: Option<&str>) -> Bytes {
    let mut out = String::new();

    if let Some(comment) = &event.comment {
        for line in comment.split('\n') {
            out.push_str(": ");
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some(name) = event.event.as_deref().or(default_event) {
        out.push_str("event: ");
        // `event` is a single-line field: drop CR/LF so a value taken from user
        // input cannot inject extra SSE fields or terminate the event early.
        push_single_line(&mut out, name, false);
        out.push('\n');
    }
    if let Some(id) = &event.id {
        out.push_str("id: ");
        // Same for `id`; the SSE spec also forbids NUL in the last-event-id.
        push_single_line(&mut out, id, true);
        out.push('\n');
    }
    if let Some(retry) = event.retry {
        out.push_str("retry: ");
        out.push_str(&retry.to_string());
        out.push('\n');
    }
    if let Some(data) = &event.data {
        for line in data.split('\n') {
            out.push_str("data: ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('\n');

    Bytes::from(out)
}

/// Appends a single-line SSE field value, dropping line terminators (`\r`, `\n`)
/// and, when `strip_nul` is set, NUL — the characters that would let a
/// user-controlled `event`/`id` inject additional fields or break the framing.
fn push_single_line(out: &mut String, value: &str, strip_nul: bool) {
    for ch in value.chars() {
        if ch == '\r' || ch == '\n' || (strip_nul && ch == '\0') {
            continue;
        }
        out.push(ch);
    }
}

/// Configuration for an SSE response.
struct SseConfig {
    default_event: Option<String>,
    heartbeat: Option<Duration>,
    no_cache: bool,
    disable_proxy_buffering: bool,
    max_event_size: Option<usize>,
    client_timeout: Option<Duration>,
    done_event: Option<String>,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            default_event: None,
            heartbeat: Some(DEFAULT_HEARTBEAT),
            no_cache: true,
            disable_proxy_buffering: true,
            max_event_size: Some(DEFAULT_MAX_EVENT_SIZE),
            client_timeout: None,
            done_event: None,
        }
    }
}

/// A streaming `text/event-stream` response carrying events of data type `T`.
pub struct Sse<T> {
    events: BoxStream<'static, Result<RawEvent>>,
    config: SseConfig,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Serialize + Send + 'static> Sse<T> {
    /// Builds an SSE response from a stream of data items.
    ///
    /// Each item becomes a `data:` event; its event name comes from
    /// [`event`](Sse::event) (or the `#[sse(event = ...)]` attribute).
    pub fn new<S>(stream: S) -> Self
    where
        S: futures_core::Stream<Item = Result<T>> + Send + 'static,
    {
        let events = stream
            .map(|item| item.and_then(|value| SseEvent::new(value).into_raw()))
            .boxed();
        Self::from_events(events)
    }

    /// Builds an SSE response from a stream of fully-specified events.
    pub fn events<S>(stream: S) -> Self
    where
        S: futures_core::Stream<Item = Result<SseEvent<T>>> + Send + 'static,
    {
        let events = stream.map(|item| item.and_then(SseEvent::into_raw)).boxed();
        Self::from_events(events)
    }

    /// Shared constructor over an already-erased event stream.
    fn from_events(events: BoxStream<'static, Result<RawEvent>>) -> Self {
        Self {
            events,
            config: SseConfig::default(),
            _marker: PhantomData,
        }
    }

    /// Sets the default event name for data items without one.
    pub fn event(mut self, default: impl Into<String>) -> Self {
        self.config.default_event = Some(default.into());
        self
    }

    /// Sets the heartbeat interval (default 15 seconds).
    pub fn heartbeat(mut self, every: Duration) -> Self {
        self.config.heartbeat = Some(every);
        self
    }

    /// Disables the heartbeat entirely.
    pub fn no_heartbeat(mut self) -> Self {
        self.config.heartbeat = None;
        self
    }

    /// Controls the `Cache-Control: no-cache` header (default on).
    pub fn no_cache(mut self, on: bool) -> Self {
        self.config.no_cache = on;
        self
    }

    /// Controls the `X-Accel-Buffering: no` header for proxies (default on).
    pub fn disable_proxy_buffering(mut self, on: bool) -> Self {
        self.config.disable_proxy_buffering = on;
        self
    }

    /// Drops events whose encoded size exceeds `bytes` (logged, not sent).
    pub fn max_event_size(mut self, bytes: usize) -> Self {
        self.config.max_event_size = Some(bytes);
        self
    }

    /// Ends the stream after `after` elapses, regardless of the source.
    pub fn client_timeout(mut self, after: Duration) -> Self {
        self.config.client_timeout = Some(after);
        self
    }

    /// Emits a final raw `data:` event when the source stream ends.
    pub fn done_event(mut self, marker: impl Into<String>) -> Self {
        self.config.done_event = Some(marker.into());
        self
    }
}

/// A concurrency cap for Server-Sent Events streams.
///
/// Each live stream holds one permit for its lifetime; the permit is released when
/// the [`SseBody`] is dropped. Registered in app state by
/// [`App::max_sse_connections`](crate::App::max_sse_connections).
#[derive(Clone)]
pub(crate) struct SseLimiter {
    semaphore: Arc<Semaphore>,
}

impl SseLimiter {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
        }
    }

    /// Reserves a connection slot, or `None` if the app is already at the cap.
    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.semaphore).try_acquire_owned().ok()
    }
}

/// Turns an [`Sse`] into a response, enforcing the app's SSE connection cap.
///
/// Used by the `#[sse]` / `#[post_sse]` generated handlers. When the app has a
/// limit configured and it is exhausted, this returns `503 Service Unavailable`
/// instead of opening another stream; otherwise the acquired permit is held by the
/// response body and released when the stream ends.
#[doc(hidden)]
pub fn __sse_into_response<T>(ctx: &RequestContext, sse: Sse<T>) -> Result<Response> {
    let permit = match ctx.state().get::<SseLimiter>() {
        Some(limiter) => match limiter.try_acquire() {
            Some(permit) => Some(permit),
            None => {
                return Err(Error::service_unavailable(
                    "the server is at its Server-Sent Events connection limit",
                ));
            }
        },
        None => None,
    };
    Ok(sse.into_response_with_permit(permit))
}

impl<T> IntoResponse for Sse<T> {
    fn into_response(self) -> Response {
        self.into_response_with_permit(None)
    }
}

impl<T> Sse<T> {
    /// Builds the streaming response, optionally holding an SSE connection permit
    /// for the lifetime of the stream.
    fn into_response_with_permit(self, permit: Option<OwnedSemaphorePermit>) -> Response {
        let Sse { events, config, .. } = self;

        let heartbeat = config
            .heartbeat
            .map(|every| interval_at(Instant::now() + every, every));
        let timeout = config.client_timeout.map(|after| Box::pin(sleep(after)));
        let done = config.done_event.map(|marker| {
            encode_event(
                &RawEvent {
                    data: Some(marker),
                    event: None,
                    id: None,
                    retry: None,
                    comment: None,
                },
                config.default_event.as_deref(),
            )
        });

        let body = SseBody {
            events,
            default_event: config.default_event,
            max_event_size: config.max_event_size,
            heartbeat,
            timeout,
            done,
            finished: false,
            _permit: permit,
        };

        let mut response = http::Response::new(RespBody::stream(body));
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(TEXT_EVENT_STREAM));
        if config.no_cache {
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
        if config.disable_proxy_buffering {
            headers.insert(
                HeaderName::from_static(X_ACCEL_BUFFERING),
                HeaderValue::from_static("no"),
            );
        }
        response
    }
}

/// The streaming body behind an [`Sse`] response.
///
/// Yields encoded events from the source stream, interleaves heartbeats while the
/// source is idle, emits an optional `done` event at the end, and stops at the
/// optional client timeout.
struct SseBody {
    events: BoxStream<'static, Result<RawEvent>>,
    default_event: Option<String>,
    max_event_size: Option<usize>,
    heartbeat: Option<Interval>,
    timeout: Option<Pin<Box<Sleep>>>,
    done: Option<Bytes>,
    finished: bool,
    /// Held for the stream's lifetime; releases the SSE connection slot on drop.
    _permit: Option<OwnedSemaphorePermit>,
}

impl SseBody {
    /// Returns the pending `done` event (if any) as the final frame.
    fn finish(&mut self) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        self.finished = true;
        Poll::Ready(self.done.take().map(|bytes| Ok(Frame::data(bytes))))
    }
}

impl Body for SseBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }

        // A reached client timeout ends the stream (after any done event).
        if let Some(timeout) = &mut this.timeout {
            if timeout.as_mut().poll(cx).is_ready() {
                return this.finish();
            }
        }

        // Drain ready events, skipping any that exceed the size limit.
        loop {
            match this.events.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    let bytes = encode_event(&event, this.default_event.as_deref());
                    if let Some(max) = this.max_event_size {
                        if bytes.len() > max {
                            tracing::warn!(
                                target: "tork",
                                event_bytes = bytes.len(),
                                max_event_size = max,
                                "SSE event exceeds max_event_size, skipping"
                            );
                            continue;
                        }
                    }
                    return Poll::Ready(Some(Ok(Frame::data(bytes))));
                }
                Poll::Ready(Some(Err(error))) => {
                    // The status is already committed; log and end the stream.
                    tracing::error!(target: "tork", error = %error, "SSE stream error");
                    return this.finish();
                }
                Poll::Ready(None) => return this.finish(),
                Poll::Pending => break,
            }
        }

        // The source is idle: send a heartbeat if one is due.
        if let Some(heartbeat) = &mut this.heartbeat {
            if heartbeat.poll_tick(cx).is_ready() {
                return Poll::Ready(Some(Ok(Frame::data(HEARTBEAT_BYTES.clone()))));
            }
        }

        Poll::Pending
    }

    fn is_end_stream(&self) -> bool {
        self.finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::time::Duration;

    fn encode<T: Serialize>(event: SseEvent<T>, default: Option<&str>) -> String {
        let raw = event.into_raw().expect("serialize");
        String::from_utf8(encode_event(&raw, default).to_vec()).unwrap()
    }

    #[derive(Debug)]
    struct BadSerialize;

    impl Serialize for BadSerialize {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("nope"))
        }
    }

    #[test]
    fn encodes_event_id_retry_and_data() {
        let text = encode(
            SseEvent::new(json!({ "id": 1 }))
                .event("item")
                .id(7)
                .retry_ms(5000),
            None,
        );
        assert_eq!(
            text,
            "event: item\nid: 7\nretry: 5000\ndata: {\"id\":1}\n\n"
        );
    }

    #[test]
    fn encodes_raw_data_with_event() {
        let text = encode(SseEvent::<()>::raw("[DONE]").event("done"), None);
        assert_eq!(text, "event: done\ndata: [DONE]\n\n");
    }

    #[test]
    fn falls_back_to_the_default_event_name() {
        let text = encode(SseEvent::new(json!(1)), Some("tick"));
        assert_eq!(text, "event: tick\ndata: 1\n\n");
    }

    #[test]
    fn comment_and_multiline_raw_data_split_into_lines() {
        let text = encode(SseEvent::<()>::raw("a\nb").comment("note"), None);
        assert_eq!(text, ": note\ndata: a\ndata: b\n\n");
    }

    #[test]
    fn event_name_and_id_cannot_inject_extra_fields() {
        // A user-controlled event name / id carrying CR/LF (or NUL in the id) must
        // not be able to inject new SSE fields or terminate the event early.
        let text = encode(
            SseEvent::new(json!(1))
                .event("ping\nevent: admin\ndata: spoofed")
                .id("9\r\nid: 0\0"),
            None,
        );
        // The line breaks are stripped, so the value stays on its own single line.
        assert_eq!(text, "event: pingevent: admindata: spoofed\nid: 9id: 0\ndata: 1\n\n");
        // Crucially: no injected blank line / second event, and the only real
        // field lines are the ones the encoder wrote itself.
        assert_eq!(text.matches("\n\n").count(), 1, "exactly one event terminator");
        assert_eq!(text.lines().filter(|l| l.starts_with("event: ")).count(), 1);
        assert_eq!(text.lines().filter(|l| l.starts_with("id: ")).count(), 1);
    }

    #[test]
    fn serialize_error_is_reported_for_typed_sse_events() {
        let error = match SseEvent::new(BadSerialize).into_raw() {
            Ok(_) => panic!("expected serialization to fail"),
            Err(error) => error,
        };
        assert!(error.message().starts_with("failed to serialize SSE data:"));
    }

    #[tokio::test]
    async fn builder_flags_toggle_headers_and_timeout_defaults() {
        let stream = futures_util::stream::pending::<Result<serde_json::Value>>();
        let response = Sse::new(stream)
            .event("tick")
            .no_cache(false)
            .disable_proxy_buffering(false)
            .no_heartbeat()
            .client_timeout(Duration::from_millis(20))
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_TYPE).is_some());
        assert!(response.headers().get(CACHE_CONTROL).is_none());
        assert!(response.headers().get(X_ACCEL_BUFFERING).is_none());
    }

    #[tokio::test]
    async fn client_timeout_finishes_without_emitting_a_done_event() {
        let stream = futures_util::stream::pending::<Result<serde_json::Value>>();
        let response = Sse::new(stream)
            .client_timeout(Duration::from_millis(20))
            .into_response();
        let mut body = response.into_body();

        let frame = tokio::time::timeout(Duration::from_secs(1), body.frame())
            .await
            .expect("timeout should trigger");
        assert!(frame.is_none());
    }

    #[tokio::test]
    async fn events_builder_handles_prebuilt_events() {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, Error>(SseEvent::new(json!({ "n": 1 })).event("tick")),
            Ok(SseEvent::raw("[DONE]").comment("final")),
        ]);
        let response = Sse::events(stream)
            .event("default")
            .done_event("[END]")
            .into_response();

        let body = body_to_string(response).await;
        assert!(
            body.contains("event: tick\ndata: {\"n\":1}\n\n"),
            "body: {body}"
        );
        assert!(body.contains(": final"), "body: {body}");
        assert!(body.contains("data: [DONE]"), "body: {body}");
        assert!(
            body.trim_end().ends_with("data: [END]"),
            "done last: {body}"
        );
    }

    async fn body_to_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn into_response_sets_headers_and_streams_events() {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, Error>(json!({ "n": 1 })),
            Ok(json!({ "n": 2 })),
        ]);
        let response = Sse::new(stream)
            .event("tick")
            .done_event("[DONE]")
            .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            TEXT_EVENT_STREAM
        );
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-cache");
        assert_eq!(response.headers().get(X_ACCEL_BUFFERING).unwrap(), "no");

        let body = body_to_string(response).await;
        assert!(
            body.contains("event: tick\ndata: {\"n\":1}\n\n"),
            "body: {body}"
        );
        assert!(
            body.contains("event: tick\ndata: {\"n\":2}\n\n"),
            "body: {body}"
        );
        assert!(
            body.trim_end().ends_with("data: [DONE]"),
            "done last: {body}"
        );
    }

    #[tokio::test]
    async fn oversized_events_are_skipped() {
        let stream = futures_util::stream::iter(vec![
            Ok::<_, Error>(json!("tiny")),
            Ok(json!(
                "a really long value that exceeds the configured maximum size"
            )),
        ]);
        let response = Sse::new(stream).max_event_size(40).into_response();
        let body = body_to_string(response).await;

        assert!(body.contains("data: \"tiny\""), "small kept: {body}");
        assert!(!body.contains("really long"), "large skipped: {body}");
    }

    #[tokio::test]
    async fn heartbeat_fires_while_the_source_is_idle() {
        // A source that never yields, so only heartbeats flow.
        let stream = futures_util::stream::pending::<Result<serde_json::Value>>();
        let response = Sse::new(stream)
            .heartbeat(Duration::from_millis(20))
            .into_response();
        let mut body = response.into_body();

        // The first frame to arrive is a heartbeat, once the interval elapses.
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("a heartbeat should arrive")
            .unwrap()
            .unwrap();
        assert_eq!(
            frame.into_data().unwrap(),
            Bytes::from_static(HEARTBEAT_FRAME)
        );
    }

    #[test]
    fn sse_limiter_caps_concurrent_permits_and_frees_them_on_drop() {
        let limiter = SseLimiter::new(2);

        let first = limiter.try_acquire().expect("first is under the cap");
        let second = limiter.try_acquire().expect("second reaches the cap");
        assert!(limiter.try_acquire().is_none(), "third is over the cap");

        // Dropping a live stream's permit frees its slot for a new connection.
        drop(first);
        let third = limiter.try_acquire().expect("a freed slot is reusable");

        drop(second);
        drop(third);
        assert!(limiter.try_acquire().is_some());
    }
}
