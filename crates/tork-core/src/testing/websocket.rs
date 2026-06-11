//! The in-process WebSocket test client.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::{SinkExt, StreamExt};
use http::header::{
    CONNECTION, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use super::client::{Shared, Transport};
use crate::body::box_body;
use crate::error::{Error, Result};
use crate::ws::{WsClose, WsCloseCode, WsMessage, connection_error, from_tungstenite, into_tungstenite};

/// Buffer size for the in-process duplex connecting client and server.
const WS_DUPLEX_BUFFER: usize = 64 * 1024;
/// A fixed, valid `Sec-WebSocket-Key`. The framing handshake is done in process,
/// so the value only needs to be present and well formed.
const WS_TEST_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// The client side of a test WebSocket transport.
///
/// In-process tests use a duplex stream; a real-port variant is added later.
pub(crate) enum ClientIo {
    Duplex(DuplexStream),
}

impl AsyncRead for ClientIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ClientIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

/// Builds a WebSocket connection: set headers, query parameters, and
/// subprotocols, then call [`connect`](TestWebSocketBuilder::connect).
pub struct TestWebSocketBuilder {
    shared: Arc<Shared>,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(HeaderName, HeaderValue)>,
    subprotocols: Vec<String>,
}

impl TestWebSocketBuilder {
    pub(crate) fn new(shared: Arc<Shared>, path: impl Into<String>) -> Self {
        Self {
            shared,
            path: path.into(),
            query: Vec::new(),
            headers: Vec::new(),
            subprotocols: Vec::new(),
        }
    }

    /// Adds a header to the upgrade request.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            self.headers.push((name, value));
        }
        self
    }

    /// Adds a query parameter to the upgrade request.
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Requests a subprotocol (sent in `Sec-WebSocket-Protocol`).
    pub fn subprotocol(mut self, protocol: &str) -> Self {
        self.subprotocols.push(protocol.to_owned());
        self
    }

    /// Performs the upgrade and returns the open connection.
    ///
    /// Returns an error if the handshake or a dependency is rejected before the
    /// upgrade (the response status is not `101`).
    pub async fn connect(self) -> Result<TestWebSocket> {
        let uri = if self.query.is_empty() {
            self.path.clone()
        } else {
            let encoded = serde_urlencoded::to_string(&self.query)
                .map_err(|_| Error::internal("failed to encode query parameters"))?;
            format!("{}?{}", self.path, encoded)
        };

        let mut request = http::Request::new(box_body(http_body_util::Full::new(bytes::Bytes::new())));
        *request.method_mut() = Method::GET;
        *request.uri_mut() = uri
            .parse()
            .map_err(|_| Error::bad_request(format!("invalid request URI: {uri}")))?;

        let map = request.headers_mut();
        for (name, value) in self.shared.default_headers.iter() {
            map.insert(name, value.clone());
        }
        for (name, value) in self.headers {
            map.insert(name, value);
        }
        self.shared
            .cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .apply(map);
        map.insert(UPGRADE, HeaderValue::from_static("websocket"));
        map.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        map.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        map.insert(SEC_WEBSOCKET_KEY, HeaderValue::from_static(WS_TEST_KEY));
        if !self.subprotocols.is_empty() {
            if let Ok(value) = HeaderValue::from_str(&self.subprotocols.join(", ")) {
                map.insert(SEC_WEBSOCKET_PROTOCOL, value);
            }
        }

        match &self.shared.transport {
            Transport::InProcess(app) => {
                let (client_io, server_io) = tokio::io::duplex(WS_DUPLEX_BUFFER);
                let response = app.dispatch_upgrade(request, server_io).await;
                if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                    return Err(Error::bad_request(format!(
                        "websocket upgrade rejected with status {}",
                        response.status()
                    ))
                    .with_code("WS_UPGRADE_REJECTED"));
                }
                let stream =
                    WebSocketStream::from_raw_socket(ClientIo::Duplex(client_io), Role::Client, None)
                        .await;
                Ok(TestWebSocket { stream })
            }
        }
    }
}

/// An open WebSocket connection in a test.
pub struct TestWebSocket {
    stream: WebSocketStream<ClientIo>,
}

impl TestWebSocket {
    /// Sends a text message.
    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<()> {
        self.send(WsMessage::Text(text.into())).await
    }

    /// Serializes `value` as JSON and sends it as a text message.
    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let text = serde_json::to_string(value)
            .map_err(|error| Error::internal(format!("failed to encode message: {error}")))?;
        self.send_text(text).await
    }

    /// Sends a binary message.
    pub async fn send_binary(&mut self, bytes: impl Into<Vec<u8>>) -> Result<()> {
        self.send(WsMessage::Binary(bytes.into())).await
    }

    async fn send(&mut self, message: WsMessage) -> Result<()> {
        self.stream
            .send(into_tungstenite(message))
            .await
            .map_err(connection_error)
    }

    /// Receives the next message, or `None` once the connection closes.
    pub async fn receive(&mut self) -> Result<Option<WsMessage>> {
        loop {
            match self.stream.next().await {
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

    /// Receives the next text message, skipping control frames.
    pub async fn receive_text(&mut self) -> Result<String> {
        loop {
            match self.receive().await? {
                Some(WsMessage::Text(text)) => return Ok(text),
                Some(WsMessage::Close(_)) | None => {
                    return Err(closed_error());
                }
                Some(_) => continue,
            }
        }
    }

    /// Receives the next message and deserializes it from JSON.
    pub async fn receive_json<T: DeserializeOwned>(&mut self) -> Result<T> {
        loop {
            match self.receive().await? {
                Some(WsMessage::Text(text)) => {
                    return serde_json::from_str(&text).map_err(decode_error);
                }
                Some(WsMessage::Binary(bytes)) => {
                    return serde_json::from_slice(&bytes).map_err(decode_error);
                }
                Some(WsMessage::Close(_)) | None => return Err(closed_error()),
                Some(_) => continue,
            }
        }
    }

    /// Waits for the close frame and returns it.
    pub async fn receive_close(&mut self) -> Result<WsClose> {
        loop {
            match self.receive().await? {
                Some(WsMessage::Close(Some(close))) => return Ok(close),
                Some(WsMessage::Close(None)) => {
                    return Ok(WsClose {
                        code: WsCloseCode::NormalClosure,
                        reason: String::new(),
                    });
                }
                None => return Err(closed_error()),
                Some(_) => continue,
            }
        }
    }

    /// Closes the connection.
    pub async fn close(&mut self) -> Result<()> {
        SinkExt::close(&mut self.stream)
            .await
            .map_err(connection_error)
    }
}

/// The error returned when the connection closed before a message was received.
fn closed_error() -> Error {
    Error::internal("websocket connection closed").with_code("WS_CLOSED")
}

/// Maps a JSON decode failure to an error.
fn decode_error(error: serde_json::Error) -> Error {
    Error::internal(format!("message is not valid JSON: {error}"))
}
