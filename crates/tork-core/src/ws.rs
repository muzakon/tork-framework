//! WebSocket connections.
//!
//! A `#[websocket]` handler receives a [`WebSocket`] handle and calls
//! [`accept`](WebSocket::accept) to obtain a live [`WebSocketConn`]. Dependencies
//! and the handshake are resolved before the upgrade, so a failure is rejected
//! with a normal HTTP response; once accepted, the connection exchanges
//! [`WsMessage`] values until it closes. The wire protocol is handled by
//! `tokio-tungstenite`, which users never see directly.

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use garde::Validate;
use http::header::{
    CONNECTION, HOST, ORIGIN, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
    UPGRADE,
};
use http::Method;
use http::{HeaderValue, StatusCode};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as TgCloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig as TgWebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::body::RespBody;
use crate::error::{Error, Result};
use crate::extract::{scheme_from_extensions, RequestContext, RequestScheme};
use crate::response::Response;
use crate::router::BoxFuture;

/// The supported WebSocket protocol version.
const WEBSOCKET_VERSION: &str = "13";
/// Error code used when a request is not a valid WebSocket upgrade.
const NOT_A_WEBSOCKET: &str = "NOT_A_WEBSOCKET";
/// Header consulted to correlate a connection with a request identifier.
const REQUEST_ID_HEADER: &str = "x-request-id";
/// Default time allowed for the upgrade handshake to complete before the
/// pending connection is abandoned, so a stalled client cannot hold a slot.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A WebSocket close status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsCloseCode {
    /// `1000` Normal closure.
    NormalClosure,
    /// `1001` The endpoint is going away.
    GoingAway,
    /// `1002` Protocol error.
    ProtocolError,
    /// `1003` Unsupported data type.
    UnsupportedData,
    /// `1008` A message violated the endpoint's policy.
    PolicyViolation,
    /// `1009` A message was too big to process.
    MessageTooBig,
    /// `1011` The server encountered an internal error.
    InternalError,
    /// Any other status code.
    Other(u16),
}

impl WsCloseCode {
    /// Returns the numeric status code.
    pub fn as_u16(self) -> u16 {
        match self {
            WsCloseCode::NormalClosure => 1000,
            WsCloseCode::GoingAway => 1001,
            WsCloseCode::ProtocolError => 1002,
            WsCloseCode::UnsupportedData => 1003,
            WsCloseCode::PolicyViolation => 1008,
            WsCloseCode::MessageTooBig => 1009,
            WsCloseCode::InternalError => 1011,
            WsCloseCode::Other(code) => code,
        }
    }

    /// Builds a close code from its numeric value.
    pub fn from_u16(code: u16) -> Self {
        match code {
            1000 => WsCloseCode::NormalClosure,
            1001 => WsCloseCode::GoingAway,
            1002 => WsCloseCode::ProtocolError,
            1003 => WsCloseCode::UnsupportedData,
            1008 => WsCloseCode::PolicyViolation,
            1009 => WsCloseCode::MessageTooBig,
            1011 => WsCloseCode::InternalError,
            other => WsCloseCode::Other(other),
        }
    }
}

/// A close control frame: a status code and a human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsClose {
    /// The close status code.
    pub code: WsCloseCode,
    /// The reason for closing.
    pub reason: String,
}

/// A WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    /// A UTF-8 text message.
    Text(String),
    /// A binary message.
    Binary(Vec<u8>),
    /// A ping control frame.
    Ping(Vec<u8>),
    /// A pong control frame.
    Pong(Vec<u8>),
    /// A close control frame, with an optional reason.
    Close(Option<WsClose>),
}

/// An error raised while handling a WebSocket connection.
///
/// Before the connection is accepted it converts into an HTTP error (so a guard
/// can reject the upgrade); after accept, prefer [`WebSocketConn::close`].
#[derive(Debug, Clone)]
pub struct WsError {
    code: WsCloseCode,
    message: String,
}

impl WsError {
    /// Creates an error with an explicit close code.
    pub fn new(code: WsCloseCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Creates a `PolicyViolation` (`1008`) error.
    pub fn policy_violation(message: impl Into<String>) -> Self {
        Self::new(WsCloseCode::PolicyViolation, message)
    }

    /// Creates an `InternalError` (`1011`) error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(WsCloseCode::InternalError, message)
    }

    /// Returns the close code this error maps to.
    pub fn code(&self) -> WsCloseCode {
        self.code
    }

    /// Returns the error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WsError {}

impl From<WsError> for Error {
    fn from(error: WsError) -> Self {
        // Used when a guard rejects the upgrade before it is accepted.
        match error.code {
            WsCloseCode::PolicyViolation => Error::forbidden(error.message),
            WsCloseCode::MessageTooBig => Error::payload_too_large(error.message),
            _ => Error::bad_request(error.message),
        }
        .with_code("WS_REJECTED")
    }
}

/// Limits and timeouts for a WebSocket connection.
///
/// Set defaults for the whole app with
/// [`App::websocket_config`](crate::App::websocket_config), or per route with the
/// `#[websocket(...)]` attributes; a route value overrides the app default.
#[derive(Clone, Default)]
pub struct WebSocketConfig {
    max_message_size: Option<usize>,
    max_frame_size: Option<usize>,
    idle_timeout: Option<Duration>,
    handshake_timeout: Option<Duration>,
    max_connections_per_ip: Option<usize>,
    origin_policy: Option<WsOriginPolicy>,
}

#[derive(Clone)]
enum WsOriginPolicy {
    Any,
    Allowlist(Vec<String>),
}

impl WebSocketConfig {
    /// Creates an empty configuration (all limits unset).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum size of an incoming message, in bytes.
    pub fn max_message_size(mut self, bytes: usize) -> Self {
        self.max_message_size = Some(bytes);
        self
    }

    /// Sets the maximum size of an incoming message, in kibibytes.
    pub fn max_message_size_kb(self, kb: usize) -> Self {
        self.max_message_size(kb * 1024)
    }

    /// Sets the maximum size of a single incoming frame, in bytes.
    pub fn max_frame_size(mut self, bytes: usize) -> Self {
        self.max_frame_size = Some(bytes);
        self
    }

    /// Sets the maximum size of a single incoming frame, in kibibytes.
    pub fn max_frame_size_kb(self, kb: usize) -> Self {
        self.max_frame_size(kb * 1024)
    }

    /// Closes the connection if no message arrives within `timeout`.
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    /// Closes the connection if no message arrives within `secs` seconds.
    pub fn idle_timeout_secs(self, secs: u64) -> Self {
        self.idle_timeout(Duration::from_secs(secs))
    }

    /// Sets how long the upgrade handshake may take before the pending
    /// connection is abandoned (default 10 seconds). Guards against a slow client
    /// that opens the upgrade and then stalls, holding a connection slot.
    pub fn handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = Some(timeout);
        self
    }

    /// Limits the number of concurrent WebSocket connections from a single client
    /// IP; further connections from that IP are rejected with `429`. An
    /// application-level setting (`App::websocket_config`).
    pub fn max_connections_per_ip(mut self, max: usize) -> Self {
        self.max_connections_per_ip = Some(max);
        self
    }

    /// Allows a browser `Origin` for this WebSocket endpoint.
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        match &mut self.origin_policy {
            Some(WsOriginPolicy::Allowlist(allowed)) => allowed.push(origin.into()),
            _ => self.origin_policy = Some(WsOriginPolicy::Allowlist(vec![origin.into()])),
        }
        self
    }

    /// Allows any browser `Origin`.
    pub fn allow_any_origin(mut self) -> Self {
        self.origin_policy = Some(WsOriginPolicy::Any);
        self
    }

    /// Returns a copy with each unset field taken from `base` (route over app).
    pub(crate) fn merge(self, base: &WebSocketConfig) -> Self {
        Self {
            max_message_size: self.max_message_size.or(base.max_message_size),
            max_frame_size: self.max_frame_size.or(base.max_frame_size),
            idle_timeout: self.idle_timeout.or(base.idle_timeout),
            handshake_timeout: self.handshake_timeout.or(base.handshake_timeout),
            max_connections_per_ip: self.max_connections_per_ip.or(base.max_connections_per_ip),
            origin_policy: self.origin_policy.or_else(|| base.origin_policy.clone()),
        }
    }

    /// The configured per-IP connection cap, if any.
    pub(crate) fn ip_connection_limit(&self) -> Option<usize> {
        self.max_connections_per_ip
    }

    /// Maps the size limits onto a tungstenite config, or `None` if both unset.
    fn to_tungstenite(&self) -> Option<TgWebSocketConfig> {
        if self.max_message_size.is_none() && self.max_frame_size.is_none() {
            return None;
        }
        Some(TgWebSocketConfig {
            max_message_size: self.max_message_size,
            max_frame_size: self.max_frame_size,
            ..TgWebSocketConfig::default()
        })
    }
}

/// The application-wide default WebSocket configuration, stored in the state map.
#[derive(Clone)]
pub(crate) struct AppWsConfig(pub(crate) WebSocketConfig);

/// A receiver, shared via the state map, that flips to `true` when the server
/// begins a graceful shutdown so live WebSocket connections can close cleanly.
#[derive(Clone)]
pub(crate) struct WsShutdown(pub(crate) watch::Receiver<bool>);

/// Tracks live WebSocket connections per client IP to cap how many a single
/// client may hold open. Shared app-wide via the state map.
#[derive(Clone)]
pub(crate) struct WsIpLimiter {
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    max: usize,
}

impl WsIpLimiter {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            counts: Arc::new(Mutex::new(HashMap::new())),
            max,
        }
    }

    /// Reserves a connection slot for `ip`, returning a permit that releases it on
    /// drop, or `None` if the client already holds the maximum.
    fn try_acquire(&self, ip: IpAddr) -> Option<WsIpPermit> {
        let mut counts = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.max {
            return None;
        }
        *count += 1;
        Some(WsIpPermit {
            counts: Arc::clone(&self.counts),
            ip,
        })
    }
}

/// Releases an IP's reserved connection slot when the connection ends.
struct WsIpPermit {
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    ip: IpAddr,
}

impl Drop for WsIpPermit {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(count) = counts.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

/// Connection metadata shared by the lifecycle events.
#[derive(Clone)]
pub(crate) struct WsConnInfo {
    method: Method,
    path: String,
    request_id: Option<String>,
}

/// Context for [`on_ws_connect`](crate::App::on_ws_connect): a socket opened.
pub struct WsConnectInfo {
    info: WsConnInfo,
}

impl WsConnectInfo {
    pub(crate) fn new(info: WsConnInfo) -> Self {
        Self { info }
    }

    /// The HTTP method of the upgrade request.
    pub fn method(&self) -> &Method {
        &self.info.method
    }

    /// The request path.
    pub fn path(&self) -> &str {
        &self.info.path
    }

    /// The request identifier (the `x-request-id` value), if present.
    pub fn request_id(&self) -> Option<&str> {
        self.info.request_id.as_deref()
    }
}

/// Context for [`on_ws_disconnect`](crate::App::on_ws_disconnect): a socket closed.
pub struct WsDisconnectInfo {
    info: WsConnInfo,
    duration: Duration,
    close_code: Option<WsCloseCode>,
}

impl WsDisconnectInfo {
    pub(crate) fn new(
        info: WsConnInfo,
        duration: Duration,
        close_code: Option<WsCloseCode>,
    ) -> Self {
        Self {
            info,
            duration,
            close_code,
        }
    }

    /// The HTTP method of the upgrade request.
    pub fn method(&self) -> &Method {
        &self.info.method
    }

    /// The request path.
    pub fn path(&self) -> &str {
        &self.info.path
    }

    /// The request identifier (the `x-request-id` value), if present.
    pub fn request_id(&self) -> Option<&str> {
        self.info.request_id.as_deref()
    }

    /// How long the connection was open.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// The close code, if the connection closed with one.
    pub fn close_code(&self) -> Option<WsCloseCode> {
        self.close_code
    }
}

/// An observe-only `on_ws_connect` hook.
pub(crate) type WsConnectHook = Box<dyn Fn(WsConnectInfo) -> BoxFuture<'static, ()> + Send + Sync>;
/// An observe-only `on_ws_disconnect` hook.
pub(crate) type WsDisconnectHook =
    Box<dyn Fn(WsDisconnectInfo) -> BoxFuture<'static, ()> + Send + Sync>;

/// The application's WebSocket lifecycle hooks, stored in the state map.
#[derive(Default)]
pub(crate) struct WsHooks {
    pub(crate) connect: Vec<WsConnectHook>,
    pub(crate) disconnect: Vec<WsDisconnectHook>,
}

/// A pending WebSocket upgrade.
///
/// Either a real upgrade negotiated by hyper on a live connection, or an
/// in-memory duplex used by the in-process test client (no network).
pub(crate) enum Upgrade {
    /// A real upgrade from hyper.
    Hyper(OnUpgrade),
    /// An in-process duplex stream (test client). Constructed by the test client,
    /// which lands in a later commit of this phase.
    #[allow(dead_code)]
    Duplex(DuplexStream),
}

/// The byte transport beneath a [`WebSocketConn`].
///
/// Both variants implement tokio's async IO traits, so the connection type stays
/// concrete while supporting a real upgraded socket and an in-process duplex.
enum WsTransport {
    Upgraded(TokioIo<Upgraded>),
    Duplex(DuplexStream),
}

impl AsyncRead for WsTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsTransport::Upgraded(io) => Pin::new(io).poll_read(cx, buf),
            WsTransport::Duplex(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for WsTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            WsTransport::Upgraded(io) => Pin::new(io).poll_write(cx, buf),
            WsTransport::Duplex(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsTransport::Upgraded(io) => Pin::new(io).poll_flush(cx),
            WsTransport::Duplex(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            WsTransport::Upgraded(io) => Pin::new(io).poll_shutdown(cx),
            WsTransport::Duplex(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

/// A WebSocket upgrade handle: call [`accept`](WebSocket::accept) to open it.
pub struct WebSocket {
    upgrade: Upgrade,
    config: WebSocketConfig,
    hooks: Arc<WsHooks>,
    info: WsConnInfo,
    permit: Option<WsIpPermit>,
    shutdown: Option<watch::Receiver<bool>>,
}

impl WebSocket {
    /// Claims the pending upgrade from the request context, merging the route's
    /// config over the application default.
    ///
    /// This is generated-code support for `#[websocket]`, not part of the
    /// everyday API. It errors (`NOT_AN_UPGRADE`) if the request is not a
    /// WebSocket upgrade.
    #[doc(hidden)]
    pub fn from_request_context(ctx: &RequestContext, route: WebSocketConfig) -> Result<Self> {
        let upgrade = ctx.take_upgrade()?;
        let app_default = ctx
            .state()
            .get::<AppWsConfig>()
            .map(|config| config.0.clone())
            .unwrap_or_default();
        let config = route.merge(&app_default);

        // Enforce the per-IP connection cap before the upgrade, so an abusive
        // client is rejected with a normal HTTP `429` rather than completing a
        // socket. The permit is held for the connection's lifetime.
        let permit = match (
            config.max_connections_per_ip,
            ctx.state().get::<WsIpLimiter>(),
            ctx.peer_addr(),
        ) {
            (Some(_), Some(limiter), Some(peer)) => {
                Some(limiter.try_acquire(peer.ip()).ok_or_else(|| {
                    Error::too_many_requests("too many WebSocket connections from this client")
                })?)
            }
            _ => None,
        };

        let hooks = ctx
            .state()
            .get::<WsHooks>()
            .unwrap_or_else(|| Arc::new(WsHooks::default()));
        let request_id = ctx
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let info = WsConnInfo {
            method: ctx.method().clone(),
            path: ctx.uri().path().to_owned(),
            request_id,
        };
        let shutdown = ctx.state().get::<WsShutdown>().map(|s| s.0.clone());
        Ok(Self {
            upgrade,
            config,
            hooks,
            info,
            permit,
            shutdown,
        })
    }

    /// Completes the upgrade and returns the live connection.
    ///
    /// Fires the `on_ws_connect` hooks once the socket is open.
    pub async fn accept(self) -> Result<WebSocketConn> {
        let idle_timeout = self.config.idle_timeout;
        let handshake_timeout = self
            .config
            .handshake_timeout
            .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT);
        let transport = match self.upgrade {
            Upgrade::Hyper(on_upgrade) => {
                // Bound the handshake so a client that stalls after starting the
                // upgrade cannot hold the pending connection open indefinitely.
                let upgraded = tokio::time::timeout(handshake_timeout, on_upgrade)
                    .await
                    .map_err(|_| Error::internal("websocket upgrade timed out"))?
                    .map_err(|error| {
                        Error::internal(format!("websocket upgrade failed: {error}"))
                    })?;
                WsTransport::Upgraded(TokioIo::new(upgraded))
            }
            Upgrade::Duplex(duplex) => WsTransport::Duplex(duplex),
        };
        let stream =
            WebSocketStream::from_raw_socket(transport, Role::Server, self.config.to_tungstenite())
                .await;

        for hook in self.hooks.connect.iter() {
            hook(WsConnectInfo::new(self.info.clone())).await;
        }

        Ok(WebSocketConn {
            stream,
            idle_timeout,
            hooks: Arc::downgrade(&self.hooks),
            info: self.info,
            started: Instant::now(),
            close_code: None,
            _permit: self.permit,
            shutdown: self.shutdown,
            hooks_fired: false,
        })
    }
}

/// A live WebSocket connection.
pub struct WebSocketConn {
    stream: WebSocketStream<WsTransport>,
    idle_timeout: Option<Duration>,
    hooks: Weak<WsHooks>,
    info: WsConnInfo,
    started: Instant,
    close_code: Option<WsCloseCode>,
    /// Held for the connection's lifetime; releases the per-IP slot on drop.
    _permit: Option<WsIpPermit>,
    /// Flips to `true` when the server starts shutting down, so [`recv`](WebSocketConn::recv)
    /// can close the connection cleanly instead of being abruptly dropped.
    shutdown: Option<watch::Receiver<bool>>,
    /// Set once the disconnect hooks have run, so they fire exactly once whether
    /// the connection closes through [`recv`](WebSocketConn::recv) or [`Drop`].
    hooks_fired: bool,
}

impl Drop for WebSocketConn {
    fn drop(&mut self) {
        let Some(hooks) = self.hooks.upgrade() else {
            return;
        };
        // The common close paths fire the hooks inline (awaited, runtime alive).
        // Drop is only the fallback when the handler dropped the socket without
        // closing it, e.g. an early return or a panic mid-stream.
        if self.hooks_fired || hooks.disconnect.is_empty() {
            return;
        }
        // Fire the disconnect hooks on a detached task (Drop cannot be async).
        // Skipped when there is no current runtime, so non-server use is safe.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let info = self.info.clone();
            let duration = self.started.elapsed();
            let close_code = self.close_code;
            handle.spawn(async move {
                for hook in hooks.disconnect.iter() {
                    hook(WsDisconnectInfo::new(info.clone(), duration, close_code)).await;
                }
            });
        }
    }
}

/// The result of one `recv` round: a frame, or a shutdown signal.
enum RecvStep {
    Shutdown,
    Frame(FrameStep),
}

/// The outcome of awaiting the next frame from the socket.
enum FrameStep {
    Message(Message),
    Error(tokio_tungstenite::tungstenite::Error),
    /// The idle timeout elapsed.
    Idle,
    /// The stream ended.
    Closed,
}

/// Awaits the next message from `stream`, honoring an optional idle timeout.
async fn next_frame(
    stream: &mut WebSocketStream<WsTransport>,
    idle_timeout: Option<Duration>,
) -> FrameStep {
    let next = match idle_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
            Ok(item) => item,
            Err(_elapsed) => return FrameStep::Idle,
        },
        None => stream.next().await,
    };
    match next {
        Some(Ok(message)) => FrameStep::Message(message),
        Some(Err(error)) => FrameStep::Error(error),
        None => FrameStep::Closed,
    }
}

impl WebSocketConn {
    /// Receives the next message, or `None` once the connection is closed.
    ///
    /// Raw frames are not surfaced; ping and pong frames are returned so the
    /// handler may observe them (the protocol layer answers pings on its own).
    pub async fn recv(&mut self) -> Result<Option<WsMessage>> {
        loop {
            // If the server is already shutting down, close cleanly right away.
            if self.shutdown.as_ref().is_some_and(|rx| *rx.borrow()) {
                let _ = self.send_close_going_away().await;
                self.fire_disconnect_hooks().await;
                return Ok(None);
            }

            let step = {
                let frame = next_frame(&mut self.stream, self.idle_timeout);
                tokio::pin!(frame);
                match &mut self.shutdown {
                    // Race the next frame against the shutdown signal.
                    Some(rx) => tokio::select! {
                        biased;
                        _ = rx.changed() => RecvStep::Shutdown,
                        outcome = &mut frame => RecvStep::Frame(outcome),
                    },
                    None => RecvStep::Frame(frame.await),
                }
            };

            match step {
                RecvStep::Shutdown => {
                    // Send a Going Away close so the client disconnects cleanly
                    // rather than seeing the socket dropped mid-shutdown.
                    let _ = self.send_close_going_away().await;
                    self.fire_disconnect_hooks().await;
                    return Ok(None);
                }
                RecvStep::Frame(FrameStep::Idle) | RecvStep::Frame(FrameStep::Closed) => {
                    self.fire_disconnect_hooks().await;
                    return Ok(None);
                }
                RecvStep::Frame(FrameStep::Error(error)) => return Err(connection_error(error)),
                RecvStep::Frame(FrameStep::Message(message)) => {
                    if let Some(message) = from_tungstenite(message) {
                        if let WsMessage::Close(close) = &message {
                            if let Some(close) = close {
                                self.close_code = Some(close.code);
                            }
                            // The peer initiated the close; fire hooks now while
                            // the runtime is alive, before the handler drops us.
                            self.fire_disconnect_hooks().await;
                        }
                        return Ok(Some(message));
                    }
                    // A control frame the protocol layer handled; keep waiting.
                }
            }
        }
    }

    /// Sends a `1001 Going Away` close frame (best effort).
    async fn send_close_going_away(&mut self) -> Result<()> {
        let close = Message::Close(Some(CloseFrame {
            code: TgCloseCode::Away,
            reason: "server shutting down".into(),
        }));
        self.stream.send(close).await.map_err(connection_error)
    }

    /// Runs the `on_ws_disconnect` hooks once, awaited in the connection's own
    /// task so they cannot be lost to a detached [`Drop`] task during shutdown.
    async fn fire_disconnect_hooks(&mut self) {
        let Some(hooks) = self.hooks.upgrade() else {
            self.hooks_fired = true;
            return;
        };
        if self.hooks_fired || hooks.disconnect.is_empty() {
            return;
        }
        self.hooks_fired = true;
        let duration = self.started.elapsed();
        for hook in hooks.disconnect.iter() {
            hook(WsDisconnectInfo::new(
                self.info.clone(),
                duration,
                self.close_code,
            ))
            .await;
        }
    }

    /// Sends a message.
    pub async fn send(&mut self, message: WsMessage) -> Result<()> {
        self.stream
            .send(into_tungstenite(message))
            .await
            .map_err(connection_error)
    }

    /// Sends a text message.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<()> {
        self.send(WsMessage::Text(text.into())).await
    }

    /// Sends a binary message.
    pub async fn send_binary(&mut self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        self.send(WsMessage::Binary(bytes.into())).await
    }

    /// Receives the next text message, skipping control frames.
    ///
    /// Returns `None` if the peer closes the connection.
    pub async fn receive_text(&mut self) -> Result<Option<String>> {
        while let Some(message) = self.recv().await? {
            match message {
                WsMessage::Text(text) => return Ok(Some(text)),
                WsMessage::Close(_) => return Ok(None),
                _ => continue,
            }
        }
        Ok(None)
    }

    /// Receives the next message and deserializes it from JSON.
    ///
    /// Accepts a text or binary payload, skips control frames, and returns `None`
    /// if the peer closes the connection. A malformed payload is a `400` error.
    pub async fn receive_json<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        while let Some(message) = self.recv().await? {
            let value = match message {
                WsMessage::Text(text) => serde_json::from_str::<T>(&text),
                WsMessage::Binary(bytes) => serde_json::from_slice::<T>(&bytes),
                WsMessage::Close(_) => return Ok(None),
                _ => continue,
            };
            return value
                .map(Some)
                .map_err(|error| Error::bad_request(format!("invalid JSON message: {error}")));
        }
        Ok(None)
    }

    /// Receives the next message, deserializes it from JSON, and validates it.
    ///
    /// Like [`receive_json`](WebSocketConn::receive_json) but also runs the
    /// type's `garde` validation; an invalid message is a `422` error whose body
    /// lists the offending fields. Returns `None` if the peer closes.
    pub async fn receive_valid<T>(&mut self) -> Result<Option<T>>
    where
        T: DeserializeOwned + Validate<Context = ()>,
    {
        while let Some(message) = self.recv().await? {
            return match message {
                WsMessage::Text(text) => deserialize_and_validate::<T>(text.as_bytes()).map(Some),
                WsMessage::Binary(bytes) => deserialize_and_validate::<T>(&bytes).map(Some),
                WsMessage::Close(_) => Ok(None),
                _ => continue,
            };
        }
        Ok(None)
    }

    /// Serializes `value` to JSON and sends it as a text message.
    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let text = serde_json::to_string(value)
            .map_err(|error| Error::internal(format!("failed to serialize message: {error}")))?;
        self.send_text(text).await
    }

    /// Closes the connection with a status code and reason.
    pub async fn close(&mut self, code: WsCloseCode, reason: impl Into<String>) -> Result<()> {
        self.close_code = Some(code);
        self.send(WsMessage::Close(Some(WsClose {
            code,
            reason: reason.into(),
        })))
        .await?;
        SinkExt::close(&mut self.stream)
            .await
            .map_err(connection_error)
    }
}

/// Validates a WebSocket handshake and builds the `101 Switching Protocols`
/// response.
///
/// This is generated-code support for `#[websocket]`, not part of the everyday
/// API. A request that is not a valid WebSocket upgrade is rejected with a
/// `400 Bad Request` (code `NOT_A_WEBSOCKET`), before the connection is opened.
#[doc(hidden)]
pub fn __ws_handshake(ctx: &RequestContext, route: WebSocketConfig) -> Result<Response> {
    validate_origin(ctx, &route)?;
    let headers = ctx.headers();

    let is_websocket = headers
        .get(UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !is_websocket {
        return Err(Error::bad_request("expected a WebSocket upgrade").with_code(NOT_A_WEBSOCKET));
    }

    let connection_upgrade = headers
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("upgrade"));
    if !connection_upgrade {
        return Err(
            Error::bad_request("WebSocket upgrade requires Connection: upgrade")
                .with_code(NOT_A_WEBSOCKET),
        );
    }

    let version_ok = headers
        .get(SEC_WEBSOCKET_VERSION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == WEBSOCKET_VERSION);
    if !version_ok {
        return Err(Error::bad_request("unsupported WebSocket version").with_code(NOT_A_WEBSOCKET));
    }

    let key = headers.get(SEC_WEBSOCKET_KEY).ok_or_else(|| {
        Error::bad_request("missing Sec-WebSocket-Key").with_code(NOT_A_WEBSOCKET)
    })?;
    let accept = derive_accept_key(key.as_bytes());
    let accept = HeaderValue::from_str(&accept)
        .map_err(|_| Error::internal("failed to build WebSocket accept header"))?;

    let mut response = http::Response::new(RespBody::new(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    Ok(response)
}

fn validate_origin(ctx: &RequestContext, route: &WebSocketConfig) -> Result<()> {
    let Some(origin) = ctx
        .headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };

    let policy = effective_config(ctx, route).origin_policy;
    match policy {
        Some(WsOriginPolicy::Any) => Ok(()),
        Some(WsOriginPolicy::Allowlist(allowed)) => {
            let actual = parse_origin(origin).ok_or_else(|| {
                Error::forbidden("websocket origin is not allowed").with_code("WS_ORIGIN_FORBIDDEN")
            })?;
            let matches = allowed
                .iter()
                .filter_map(|origin| parse_origin(origin))
                .any(|allowed| allowed == actual);
            if matches {
                Ok(())
            } else {
                Err(Error::forbidden("websocket origin is not allowed")
                    .with_code("WS_ORIGIN_FORBIDDEN"))
            }
        }
        None => {
            let actual = parse_origin(origin).ok_or_else(|| {
                Error::forbidden("websocket origin is not allowed").with_code("WS_ORIGIN_FORBIDDEN")
            })?;
            let expected = expected_same_origin(ctx).ok_or_else(|| {
                Error::forbidden("websocket origin is not allowed").with_code("WS_ORIGIN_FORBIDDEN")
            })?;
            if actual == expected {
                Ok(())
            } else {
                Err(Error::forbidden("websocket origin is not allowed")
                    .with_code("WS_ORIGIN_FORBIDDEN"))
            }
        }
    }
}

fn effective_config(ctx: &RequestContext, route: &WebSocketConfig) -> WebSocketConfig {
    let base = ctx
        .state()
        .get::<AppWsConfig>()
        .map(|config| config.0.clone())
        .unwrap_or_default();
    route.clone().merge(&base)
}

#[derive(Clone, PartialEq, Eq)]
struct ParsedOrigin {
    scheme: &'static str,
    host: String,
    port: u16,
}

fn parse_origin(origin: &str) -> Option<ParsedOrigin> {
    let uri: http::Uri = origin.parse().ok()?;
    let scheme = match uri.scheme_str()? {
        "http" => "http",
        "https" => "https",
        _ => return None,
    };
    let authority = uri.authority()?;
    Some(ParsedOrigin {
        scheme,
        host: authority.host().to_ascii_lowercase(),
        port: authority.port_u16().unwrap_or(default_port(scheme)),
    })
}

fn expected_same_origin(ctx: &RequestContext) -> Option<ParsedOrigin> {
    let scheme = scheme_from_extensions(&ctx.head().extensions)
        .unwrap_or(RequestScheme::Http)
        .as_str();
    let host = ctx.headers().get(HOST)?.to_str().ok()?;
    let authority: http::uri::Authority = host.parse().ok()?;
    Some(ParsedOrigin {
        scheme,
        host: authority.host().to_ascii_lowercase(),
        port: authority.port_u16().unwrap_or(default_port(scheme)),
    })
}

fn default_port(scheme: &str) -> u16 {
    if scheme == "https" {
        443
    } else {
        80
    }
}

/// Deserializes a JSON message and runs its `garde` validation.
fn deserialize_and_validate<T>(bytes: &[u8]) -> Result<T>
where
    T: DeserializeOwned + Validate<Context = ()>,
{
    let value: T = serde_json::from_slice(bytes)
        .map_err(|error| Error::unprocessable(format!("invalid JSON message: {error}")))?;
    value.validate().map_err(Error::from_garde_report)?;
    Ok(value)
}

/// Maps a framework message to a tungstenite message.
pub(crate) fn into_tungstenite(message: WsMessage) -> Message {
    match message {
        WsMessage::Text(text) => Message::Text(text),
        WsMessage::Binary(bytes) => Message::Binary(bytes),
        WsMessage::Ping(bytes) => Message::Ping(bytes),
        WsMessage::Pong(bytes) => Message::Pong(bytes),
        WsMessage::Close(close) => Message::Close(close.map(|close| CloseFrame {
            code: TgCloseCode::from(close.code.as_u16()),
            reason: Cow::Owned(close.reason),
        })),
    }
}

/// Maps a tungstenite message to a framework message, dropping raw frames.
pub(crate) fn from_tungstenite(message: Message) -> Option<WsMessage> {
    match message {
        Message::Text(text) => Some(WsMessage::Text(text)),
        Message::Binary(bytes) => Some(WsMessage::Binary(bytes)),
        Message::Ping(bytes) => Some(WsMessage::Ping(bytes)),
        Message::Pong(bytes) => Some(WsMessage::Pong(bytes)),
        Message::Close(close) => Some(WsMessage::Close(close.map(|close| WsClose {
            code: WsCloseCode::from_u16(u16::from(close.code)),
            reason: close.reason.into_owned(),
        }))),
        Message::Frame(_) => None,
    }
}

/// Renders a tungstenite protocol error as a framework error.
pub(crate) fn connection_error(error: tokio_tungstenite::tungstenite::Error) -> Error {
    Error::internal(format!("websocket connection error: {error}")).with_code("WS_CONNECTION_ERROR")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::extract::PathParams;
    use crate::state::StateMap;
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use http_body_util::Full;
    use std::sync::Mutex;
    use tokio_tungstenite::tungstenite::protocol::Role;

    fn request_context(headers: &[(&str, &str)]) -> RequestContext {
        let mut builder = http::Request::builder().method(Method::GET).uri("/ws");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let head = builder.body(()).unwrap().into_parts().0;
        RequestContext::new(
            head,
            PathParams::new(),
            Arc::new(StateMap::new()),
            box_body(Full::new(Bytes::new())),
        )
    }

    fn request_context_with_duplex(
        headers: &[(&str, &str)],
        config: Option<WebSocketConfig>,
        hooks: Option<WsHooks>,
    ) -> (RequestContext, DuplexStream) {
        let mut builder = http::Request::builder().method(Method::GET).uri("/ws");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let head = builder.body(()).unwrap().into_parts().0;
        let mut state = StateMap::new();
        if let Some(config) = config {
            state.insert(AppWsConfig(config));
        }
        if let Some(hooks) = hooks {
            state.insert(hooks);
        }
        let (client, server) = tokio::io::duplex(64 * 1024);
        let ctx = RequestContext::with_duplex_upgrade(
            head,
            PathParams::new(),
            Arc::new(state),
            box_body(Full::new(Bytes::new())),
            server,
        );
        (ctx, client)
    }

    fn websocket_headers() -> [(&'static str, &'static str); 4] {
        [
            ("upgrade", "websocket"),
            ("connection", "keep-alive, Upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]
    }

    fn default_route_config() -> WebSocketConfig {
        WebSocketConfig::new()
    }

    #[test]
    fn close_code_round_trips_through_u16() {
        for code in [
            WsCloseCode::NormalClosure,
            WsCloseCode::GoingAway,
            WsCloseCode::ProtocolError,
            WsCloseCode::UnsupportedData,
            WsCloseCode::PolicyViolation,
            WsCloseCode::MessageTooBig,
            WsCloseCode::InternalError,
            WsCloseCode::Other(4000),
        ] {
            assert_eq!(WsCloseCode::from_u16(code.as_u16()), code);
        }
    }

    #[test]
    fn messages_map_to_and_from_tungstenite() {
        let cases = [
            WsMessage::Text("hello".to_owned()),
            WsMessage::Binary(vec![1, 2, 3]),
            WsMessage::Ping(vec![9]),
            WsMessage::Pong(vec![8]),
            WsMessage::Close(Some(WsClose {
                code: WsCloseCode::NormalClosure,
                reason: "bye".to_owned(),
            })),
        ];
        for message in cases {
            let round = from_tungstenite(into_tungstenite(message.clone()));
            assert_eq!(round, Some(message));
        }
    }

    #[test]
    fn config_merge_prefers_route_over_app() {
        let app = WebSocketConfig::new()
            .max_message_size(1000)
            .idle_timeout_secs(30);
        let route = WebSocketConfig::new().max_message_size(2000);

        let merged = route.merge(&app);
        assert_eq!(merged.max_message_size, Some(2000), "route value wins");
        assert_eq!(merged.max_frame_size, None);
        assert_eq!(
            merged.idle_timeout,
            Some(Duration::from_secs(30)),
            "app default is kept where the route is unset"
        );
    }

    #[test]
    fn ws_error_maps_to_an_http_status() {
        let error: Error = WsError::policy_violation("no token").into();
        assert_eq!(error.kind(), crate::ErrorKind::Forbidden);
        assert_eq!(error.code(), "WS_REJECTED");

        let too_large: Error = WsError::new(WsCloseCode::MessageTooBig, "big").into();
        assert_eq!(too_large.kind(), crate::ErrorKind::PayloadTooLarge);

        let internal = WsError::internal("boom");
        assert_eq!(internal.code(), WsCloseCode::InternalError);
        assert_eq!(internal.message(), "boom");
        assert_eq!(internal.to_string(), "boom");
    }

    #[test]
    fn disconnect_info_exposes_duration_and_close_code() {
        let info = WsConnInfo {
            method: Method::GET,
            path: "/ws".to_owned(),
            request_id: Some("req-1".to_owned()),
        };
        let event = WsDisconnectInfo::new(
            info,
            Duration::from_secs(3),
            Some(WsCloseCode::NormalClosure),
        );
        assert_eq!(event.path(), "/ws");
        assert_eq!(event.method(), &Method::GET);
        assert_eq!(event.request_id(), Some("req-1"));
        assert_eq!(event.duration(), Duration::from_secs(3));
        assert_eq!(event.close_code(), Some(WsCloseCode::NormalClosure));
    }

    #[test]
    fn websocket_config_builders_and_connect_info_accessors_work() {
        let config = WebSocketConfig::new()
            .max_message_size_kb(2)
            .max_frame_size_kb(1)
            .idle_timeout_secs(3);
        let tungstenite = config.to_tungstenite().expect("limits should be present");
        assert_eq!(tungstenite.max_message_size, Some(2 * 1024));
        assert_eq!(tungstenite.max_frame_size, Some(1024));
        assert_eq!(config.idle_timeout, Some(Duration::from_secs(3)));
        assert!(WebSocketConfig::new().to_tungstenite().is_none());

        let info = WsConnInfo {
            method: Method::POST,
            path: "/chat".to_owned(),
            request_id: Some("req-9".to_owned()),
        };
        let connect = WsConnectInfo::new(info);
        assert_eq!(connect.method(), &Method::POST);
        assert_eq!(connect.path(), "/chat");
        assert_eq!(connect.request_id(), Some("req-9"));
    }

    #[test]
    fn handshake_validates_required_headers() {
        let ctx = request_context(&[]);
        let error = match __ws_handshake(&ctx, default_route_config()) {
            Ok(_) => panic!("expected handshake rejection"),
            Err(error) => error,
        };
        assert_eq!(error.code(), NOT_A_WEBSOCKET);
        assert_eq!(error.message(), "expected a WebSocket upgrade");

        let ctx = request_context(&[
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]);
        let error = match __ws_handshake(&ctx, default_route_config()) {
            Ok(_) => panic!("expected handshake rejection"),
            Err(error) => error,
        };
        assert_eq!(
            error.message(),
            "WebSocket upgrade requires Connection: upgrade"
        );

        let ctx = request_context(&[
            ("upgrade", "websocket"),
            ("connection", "upgrade"),
            ("sec-websocket-version", "12"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]);
        let error = match __ws_handshake(&ctx, default_route_config()) {
            Ok(_) => panic!("expected handshake rejection"),
            Err(error) => error,
        };
        assert_eq!(error.message(), "unsupported WebSocket version");

        let ctx = request_context(&[
            ("upgrade", "websocket"),
            ("connection", "upgrade"),
            ("sec-websocket-version", "13"),
        ]);
        let error = match __ws_handshake(&ctx, default_route_config()) {
            Ok(_) => panic!("expected handshake rejection"),
            Err(error) => error,
        };
        assert_eq!(error.message(), "missing Sec-WebSocket-Key");
    }

    #[test]
    fn handshake_builds_switching_protocols_response() {
        let ctx = request_context(&websocket_headers());
        let response = __ws_handshake(&ctx, default_route_config()).unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(response.headers()[UPGRADE], "websocket");
        assert_eq!(response.headers()[CONNECTION], "upgrade");
        assert!(response.headers().contains_key(SEC_WEBSOCKET_ACCEPT));
    }

    #[test]
    fn handshake_rejects_cross_origin_by_default_and_accepts_same_origin() {
        let ctx = request_context(&[
            ("host", "example.com"),
            ("origin", "https://evil.example.com"),
            ("upgrade", "websocket"),
            ("connection", "upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]);
        let error = match __ws_handshake(&ctx, default_route_config()) {
            Ok(_) => panic!("expected handshake rejection"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), crate::ErrorKind::Forbidden);
        assert_eq!(error.code(), "WS_ORIGIN_FORBIDDEN");

        let mut head = http::Request::builder()
            .method(Method::GET)
            .uri("/ws")
            .header("host", "example.com")
            .header("origin", "https://example.com")
            .header("upgrade", "websocket")
            .header("connection", "upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(())
            .unwrap()
            .into_parts()
            .0;
        head.extensions.insert(RequestScheme::Https);
        let ctx = RequestContext::new(
            head,
            PathParams::new(),
            Arc::new(StateMap::new()),
            box_body(Full::new(Bytes::new())),
        );
        let response = __ws_handshake(&ctx, default_route_config()).unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    }

    #[test]
    fn allowlists_and_allow_any_origin_override_same_origin_policy() {
        let ctx = request_context(&[
            ("host", "example.com"),
            ("origin", "https://evil.example.com"),
            ("upgrade", "websocket"),
            ("connection", "upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]);

        let response = __ws_handshake(
            &ctx,
            WebSocketConfig::new().allow_origin("https://evil.example.com"),
        )
        .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        let response = __ws_handshake(&ctx, WebSocketConfig::new().allow_any_origin()).unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    }

    #[test]
    fn from_request_context_merges_config_and_captures_request_metadata() {
        let hooks = WsHooks::default();
        let (ctx, _client) = request_context_with_duplex(
            &[
                ("x-request-id", "req-2"),
                ("upgrade", "websocket"),
                ("connection", "upgrade"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
            Some(WebSocketConfig::new().max_frame_size(64)),
            Some(hooks),
        );

        let socket = WebSocket::from_request_context(
            &ctx,
            WebSocketConfig::new()
                .max_message_size(128)
                .idle_timeout(Duration::from_secs(2)),
        )
        .unwrap();

        assert_eq!(socket.config.max_message_size, Some(128));
        assert_eq!(socket.config.max_frame_size, Some(64));
        assert_eq!(socket.config.idle_timeout, Some(Duration::from_secs(2)));
        assert_eq!(socket.info.path, "/ws");
        assert_eq!(socket.info.request_id.as_deref(), Some("req-2"));
        assert!(socket.hooks.connect.is_empty());
        assert!(socket.hooks.disconnect.is_empty());
    }

    #[derive(Debug, PartialEq, Eq, serde::Deserialize, garde::Validate)]
    struct ChatIn {
        #[garde(length(min = 1))]
        message: String,
    }

    #[test]
    fn deserialize_and_validate_accepts_valid_and_rejects_invalid() {
        let ok = deserialize_and_validate::<ChatIn>(br#"{"message":"hi"}"#);
        assert!(ok.is_ok());

        let empty = deserialize_and_validate::<ChatIn>(br#"{"message":""}"#);
        assert_eq!(empty.err().unwrap().kind(), crate::ErrorKind::Unprocessable);

        let malformed = deserialize_and_validate::<ChatIn>(b"not json");
        assert_eq!(
            malformed.err().unwrap().kind(),
            crate::ErrorKind::Unprocessable
        );
    }

    #[tokio::test]
    async fn duplex_accept_runs_hooks_and_exchanges_messages() {
        let connects = Arc::new(Mutex::new(Vec::new()));
        let disconnects = Arc::new(Mutex::new(Vec::new()));
        let hooks = WsHooks {
            connect: vec![Box::new({
                let connects = connects.clone();
                move |info| {
                    let connects = connects.clone();
                    Box::pin(async move {
                        connects.lock().unwrap().push((
                            info.method().clone(),
                            info.path().to_owned(),
                            info.request_id().map(str::to_owned),
                        ));
                    })
                }
            })],
            disconnect: vec![Box::new({
                let disconnects = disconnects.clone();
                move |info| {
                    let disconnects = disconnects.clone();
                    Box::pin(async move {
                        disconnects
                            .lock()
                            .unwrap()
                            .push((info.path().to_owned(), info.close_code()));
                    })
                }
            })],
        };
        let headers = [
            ("x-request-id", "req-hook"),
            ("upgrade", "websocket"),
            ("connection", "upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ];
        let (ctx, client_io) = request_context_with_duplex(&headers, None, Some(hooks));
        let socket = WebSocket::from_request_context(&ctx, WebSocketConfig::new()).unwrap();
        let mut conn = socket.accept().await.unwrap();
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;

        client.send(Message::Text("hello".into())).await.unwrap();
        assert_eq!(conn.receive_text().await.unwrap(), Some("hello".to_owned()));

        conn.send_json(&serde_json::json!({ "ok": true }))
            .await
            .unwrap();
        let message = client.next().await.unwrap().unwrap();
        assert_eq!(message.into_text().unwrap(), r#"{"ok":true}"#);

        conn.close(WsCloseCode::NormalClosure, "bye").await.unwrap();
        match client.next().await.unwrap().unwrap() {
            Message::Close(Some(close)) => {
                assert_eq!(u16::from(close.code), 1000);
                assert_eq!(close.reason, "bye");
            }
            other => panic!("expected close frame, got {other:?}"),
        }
        drop(conn);
        tokio::task::yield_now().await;

        assert_eq!(
            connects.lock().unwrap().as_slice(),
            &[(Method::GET, "/ws".to_owned(), Some("req-hook".to_owned()))]
        );
        assert_eq!(
            disconnects.lock().unwrap().as_slice(),
            &[("/ws".to_owned(), Some(WsCloseCode::NormalClosure))]
        );
    }

    #[tokio::test]
    async fn duplex_connection_helpers_cover_close_idle_and_validation_paths() {
        let (ctx, client_io) = request_context_with_duplex(&websocket_headers(), None, None);
        let socket = WebSocket::from_request_context(
            &ctx,
            WebSocketConfig::new().idle_timeout(Duration::from_millis(10)),
        )
        .unwrap();
        let mut conn = socket.accept().await.unwrap();
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;

        client.send(Message::Ping(vec![1, 2])).await.unwrap();
        client
            .send(Message::Text("{\"message\":\"ok\"}".into()))
            .await
            .unwrap();
        let validated = conn.receive_valid::<ChatIn>().await.unwrap().unwrap();
        assert_eq!(validated.message, "ok");

        client
            .send(Message::Binary(br#"{"message":""}"#.to_vec()))
            .await
            .unwrap();
        let error = match conn.receive_valid::<ChatIn>().await {
            Ok(_) => panic!("expected validation error"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), crate::ErrorKind::Unprocessable);

        client.send(Message::Text("not-json".into())).await.unwrap();
        let error = match conn.receive_json::<ChatIn>().await {
            Ok(_) => panic!("expected decode error"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), crate::ErrorKind::BadRequest);

        client.close(None).await.unwrap();
        assert_eq!(conn.receive_text().await.unwrap(), None);
        assert_eq!(conn.receive_json::<ChatIn>().await.unwrap(), None);
        assert_eq!(conn.receive_valid::<ChatIn>().await.unwrap(), None);

        let (ctx, _client_io) = request_context_with_duplex(&websocket_headers(), None, None);
        let socket = WebSocket::from_request_context(
            &ctx,
            WebSocketConfig::new().idle_timeout(Duration::from_millis(5)),
        )
        .unwrap();
        let mut idle_conn = socket.accept().await.unwrap();
        assert_eq!(idle_conn.recv().await.unwrap(), None);
    }

    #[test]
    fn frame_and_connection_errors_map_to_expected_results() {
        let error = connection_error(tokio_tungstenite::tungstenite::Error::ConnectionClosed);
        assert_eq!(error.code(), "WS_CONNECTION_ERROR");
        assert!(error.message().contains("websocket connection error:"));
    }

    #[test]
    fn ws_ip_limiter_caps_per_ip_and_releases_on_drop() {
        use std::net::Ipv4Addr;

        let limiter = WsIpLimiter::new(2);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        let first = limiter.try_acquire(ip).expect("first is under the limit");
        let _second = limiter.try_acquire(ip).expect("second reaches the limit");
        assert!(
            limiter.try_acquire(ip).is_none(),
            "a third connection from the same IP is rejected"
        );

        // A different IP has its own budget.
        let other = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(limiter.try_acquire(other).is_some());

        // Releasing a permit frees a slot for that IP again.
        drop(first);
        assert!(
            limiter.try_acquire(ip).is_some(),
            "dropping a connection frees a slot"
        );
    }

    #[test]
    fn route_config_overrides_app_defaults_for_new_limits() {
        let app = WebSocketConfig::new()
            .handshake_timeout(Duration::from_secs(5))
            .max_connections_per_ip(10);
        let route = WebSocketConfig::new().max_connections_per_ip(3);

        let merged = route.merge(&app);
        assert_eq!(merged.ip_connection_limit(), Some(3), "route wins");
        assert_eq!(
            merged.handshake_timeout,
            Some(Duration::from_secs(5)),
            "unset on the route, taken from the app default"
        );
    }
}
