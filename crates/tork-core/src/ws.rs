//! WebSocket connections.
//!
//! A `#[websocket]` handler receives a [`WebSocket`] handle and calls
//! [`accept`](WebSocket::accept) to obtain a live [`WebSocketConn`]. Dependencies
//! and the handshake are resolved before the upgrade, so a failure is rejected
//! with a normal HTTP response; once accepted, the connection exchanges
//! [`WsMessage`] values until it closes. The wire protocol is handled by
//! `tokio-tungstenite`, which users never see directly.

use std::borrow::Cow;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use garde::Validate;
use http::header::{
    CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderValue, StatusCode};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig as TgWebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as TgCloseCode;

use crate::body::RespBody;
use crate::error::{Error, Result};
use crate::extract::RequestContext;
use crate::response::Response;

/// The supported WebSocket protocol version.
const WEBSOCKET_VERSION: &str = "13";
/// Error code used when a request is not a valid WebSocket upgrade.
const NOT_A_WEBSOCKET: &str = "NOT_A_WEBSOCKET";

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

    /// Returns a copy with each unset field taken from `base` (route over app).
    pub(crate) fn merge(self, base: &WebSocketConfig) -> Self {
        Self {
            max_message_size: self.max_message_size.or(base.max_message_size),
            max_frame_size: self.max_frame_size.or(base.max_frame_size),
            idle_timeout: self.idle_timeout.or(base.idle_timeout),
        }
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

/// A WebSocket upgrade handle: call [`accept`](WebSocket::accept) to open it.
pub struct WebSocket {
    upgrade: OnUpgrade,
    config: WebSocketConfig,
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
        Ok(Self {
            upgrade,
            config: route.merge(&app_default),
        })
    }

    /// Completes the upgrade and returns the live connection.
    pub async fn accept(self) -> Result<WebSocketConn> {
        let idle_timeout = self.config.idle_timeout;
        let upgraded = self
            .upgrade
            .await
            .map_err(|error| Error::internal(format!("websocket upgrade failed: {error}")))?;
        let stream = WebSocketStream::from_raw_socket(
            TokioIo::new(upgraded),
            Role::Server,
            self.config.to_tungstenite(),
        )
        .await;
        Ok(WebSocketConn {
            stream,
            idle_timeout,
        })
    }
}

/// A live WebSocket connection.
pub struct WebSocketConn {
    stream: WebSocketStream<TokioIo<Upgraded>>,
    idle_timeout: Option<Duration>,
}

impl WebSocketConn {
    /// Receives the next message, or `None` once the connection is closed.
    ///
    /// Raw frames are not surfaced; ping and pong frames are returned so the
    /// handler may observe them (the protocol layer answers pings on its own).
    pub async fn recv(&mut self) -> Result<Option<WsMessage>> {
        loop {
            let next = match self.idle_timeout {
                // An idle period beyond the timeout ends the connection.
                Some(timeout) => match tokio::time::timeout(timeout, self.stream.next()).await {
                    Ok(item) => item,
                    Err(_elapsed) => return Ok(None),
                },
                None => self.stream.next().await,
            };
            match next {
                Some(Ok(message)) => {
                    if let Some(message) = from_tungstenite(message) {
                        return Ok(Some(message));
                    }
                }
                Some(Err(error)) => return Err(connection_error(error)),
                None => return Ok(None),
            }
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
pub fn __ws_handshake(ctx: &RequestContext) -> Result<Response> {
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
        return Err(
            Error::bad_request("unsupported WebSocket version").with_code(NOT_A_WEBSOCKET)
        );
    }

    let key = headers
        .get(SEC_WEBSOCKET_KEY)
        .ok_or_else(|| Error::bad_request("missing Sec-WebSocket-Key").with_code(NOT_A_WEBSOCKET))?;
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
fn into_tungstenite(message: WsMessage) -> Message {
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
fn from_tungstenite(message: Message) -> Option<WsMessage> {
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
fn connection_error(error: tokio_tungstenite::tungstenite::Error) -> Error {
    Error::internal(format!("websocket connection error: {error}")).with_code("WS_CONNECTION_ERROR")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[derive(serde::Deserialize, garde::Validate)]
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
        assert_eq!(malformed.err().unwrap().kind(), crate::ErrorKind::Unprocessable);
    }
}
