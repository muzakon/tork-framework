//! The in-process WebSocket test client.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::{SinkExt, StreamExt};
use http::header::{
    CONNECTION, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

use super::client::{Shared, TestHeader, Transport};
use crate::body::box_body;
use crate::error::{Error, Result};
use crate::ws::{
    connection_error, from_tungstenite, into_tungstenite, WsClose, WsCloseCode, WsMessage,
};

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
    Tcp(TcpStream),
}

impl AsyncRead for ClientIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_read(cx, buf),
            ClientIo::Tcp(io) => Pin::new(io).poll_read(cx, buf),
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
            ClientIo::Tcp(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_flush(cx),
            ClientIo::Tcp(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ClientIo::Duplex(io) => Pin::new(io).poll_shutdown(cx),
            ClientIo::Tcp(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

/// Builds a WebSocket connection: set headers, query parameters, and
/// subprotocols, then call [`connect`](TestWebSocketBuilder::connect).
pub struct TestWebSocketBuilder {
    shared: Arc<Shared>,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<TestHeader>,
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
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::safe(name, value));
        }
        self
    }

    /// Adds a security-sensitive header to the upgrade request, bypassing the
    /// in-process guard.
    pub fn unsafe_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            self.headers.push(TestHeader::unsafe_allowed(name, value));
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
        let path = if self.query.is_empty() {
            self.path.clone()
        } else {
            let encoded = serde_urlencoded::to_string(&self.query)
                .map_err(|_| Error::internal("failed to encode query parameters"))?;
            format!("{}?{}", self.path, encoded)
        };

        // The headers common to both transports: defaults, per-request, and cookies.
        let mut base_headers = HeaderMap::new();
        for (name, value) in self.shared.default_headers.iter() {
            base_headers.insert(name, value.clone());
        }
        for (name, value) in self.shared.unsafe_default_headers.iter() {
            base_headers.insert(name, value.clone());
        }
        self.shared
            .reject_in_process_sensitive_headers(&self.headers)?;
        for header in &self.headers {
            base_headers.insert(header.name.clone(), header.value.clone());
        }
        self.shared
            .cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .apply(&mut base_headers);
        let subprotocol = if self.subprotocols.is_empty() {
            None
        } else {
            HeaderValue::from_str(&self.subprotocols.join(", ")).ok()
        };

        match &self.shared.transport {
            Transport::InProcess(app) => {
                let mut request =
                    http::Request::new(box_body(http_body_util::Full::new(bytes::Bytes::new())));
                *request.method_mut() = Method::GET;
                *request.uri_mut() = path
                    .parse()
                    .map_err(|_| Error::bad_request(format!("invalid request URI: {path}")))?;
                let map = request.headers_mut();
                *map = base_headers;
                map.insert(UPGRADE, HeaderValue::from_static("websocket"));
                map.insert(CONNECTION, HeaderValue::from_static("upgrade"));
                map.insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
                map.insert(SEC_WEBSOCKET_KEY, HeaderValue::from_static(WS_TEST_KEY));
                if let Some(value) = subprotocol {
                    map.insert(SEC_WEBSOCKET_PROTOCOL, value);
                }

                let (client_io, server_io) = tokio::io::duplex(WS_DUPLEX_BUFFER);
                let response = app.dispatch_upgrade(request, server_io).await;
                if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                    return Err(rejected(response.status()));
                }
                let stream = WebSocketStream::from_raw_socket(
                    ClientIo::Duplex(client_io),
                    Role::Client,
                    None,
                )
                .await;
                Ok(TestWebSocket { stream })
            }
            Transport::RealPort(addr) => {
                // Build the handshake request from the URL so tungstenite generates
                // the mandatory headers (key, version, upgrade), then add ours.
                let url = format!("ws://{addr}{path}");
                let mut request = url
                    .as_str()
                    .into_client_request()
                    .map_err(connection_error)?;
                for (name, value) in base_headers.iter() {
                    request.headers_mut().insert(name, value.clone());
                }
                if let Some(value) = subprotocol {
                    request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, value);
                }

                let stream = TcpStream::connect(addr).await.map_err(|error| {
                    Error::internal(format!("failed to connect to {addr}: {error}"))
                })?;
                let (stream, _response) =
                    tokio_tungstenite::client_async(request, ClientIo::Tcp(stream))
                        .await
                        .map_err(connection_error)?;
                Ok(TestWebSocket { stream })
            }
        }
    }
}

/// The error returned when a WebSocket upgrade is rejected before acceptance.
fn rejected(status: StatusCode) -> Error {
    Error::bad_request(format!("websocket upgrade rejected with status {status}"))
        .with_code("WS_UPGRADE_REJECTED")
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

#[cfg(test)]
mod tests {
    use super::super::client::{Shared, Transport};
    use super::super::cookie::CookieJar;
    use super::*;
    use crate::app::App;
    use http::HeaderMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn builder_ignores_invalid_headers_and_keeps_query_and_subprotocols() {
        let shared = Arc::new(Shared {
            transport: Transport::InProcess(Arc::new(App::new().build().unwrap())),
            default_headers: HeaderMap::new(),
            unsafe_default_headers: HeaderMap::new(),
            cookies: std::sync::Mutex::new(CookieJar::default()),
        });

        let builder = TestWebSocketBuilder::new(shared, "/ws")
            .header("x-good", "ok")
            .header("bad name", "ignored")
            .header("x-bad-value", "line\nbreak")
            .query("room", "main hall")
            .subprotocol("json")
            .subprotocol("binary");

        assert_eq!(builder.headers.len(), 1);
        assert_eq!(
            builder.query,
            vec![("room".to_owned(), "main hall".to_owned())]
        );
        assert_eq!(
            builder.subprotocols,
            vec!["json".to_owned(), "binary".to_owned()]
        );
    }

    #[test]
    fn unsafe_header_marks_the_entry() {
        let shared = Arc::new(Shared {
            transport: Transport::InProcess(Arc::new(App::new().build().unwrap())),
            default_headers: HeaderMap::new(),
            unsafe_default_headers: HeaderMap::new(),
            cookies: std::sync::Mutex::new(CookieJar::default()),
        });
        let builder = TestWebSocketBuilder::new(shared, "/ws").unsafe_header("host", "example.com");
        assert_eq!(builder.headers.len(), 1);
        assert!(builder.headers[0].unsafe_allowed);
    }

    #[test]
    fn rejected_error_uses_stable_code() {
        let error = rejected(StatusCode::FORBIDDEN);

        assert_eq!(error.code(), "WS_UPGRADE_REJECTED");
        assert_eq!(
            error.message(),
            "websocket upgrade rejected with status 403 Forbidden"
        );
    }

    #[test]
    fn closed_error_uses_stable_code() {
        let error = closed_error();

        assert_eq!(error.code(), "WS_CLOSED");
        assert_eq!(error.message(), "websocket connection closed");
    }

    #[test]
    fn decode_error_reports_json_failure() {
        let source = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let error = decode_error(source);

        assert!(error.message().starts_with("message is not valid JSON:"));
    }

    #[tokio::test]
    async fn client_io_duplex_supports_async_read_and_write() {
        let (left, mut right) = tokio::io::duplex(16);
        let mut io = ClientIo::Duplex(left);

        io.write_all(b"ping").await.unwrap();
        io.flush().await.unwrap();

        let mut buf = [0u8; 4];
        right.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");

        right.write_all(b"pong").await.unwrap();
        right.flush().await.unwrap();

        let mut back = [0u8; 4];
        io.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"pong");
    }

    #[tokio::test]
    async fn client_io_tcp_supports_async_read_and_write() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            socket.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            socket.write_all(b"pong").await.unwrap();
            socket.flush().await.unwrap();
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut io = ClientIo::Tcp(stream);

        io.write_all(b"ping").await.unwrap();
        io.flush().await.unwrap();

        let mut back = [0u8; 4];
        io.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"pong");

        let _ = server.await;
    }

    #[tokio::test]
    async fn client_io_duplex_poll_shutdown_completes() {
        use tokio::io::AsyncWriteExt;
        let (left, _right) = tokio::io::duplex(16);
        let mut io = ClientIo::Duplex(left);
        // Shutdown should complete without error.
        io.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn client_io_tcp_poll_shutdown_completes() {
        use tokio::io::AsyncWriteExt;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut io = ClientIo::Tcp(stream);
        io.shutdown().await.unwrap();
        let _ = server.await;
    }

    #[test]
    fn builder_keeps_query_parameters() {
        let shared = Arc::new(Shared {
            transport: Transport::InProcess(Arc::new(App::new().build().unwrap())),
            default_headers: HeaderMap::new(),
            unsafe_default_headers: HeaderMap::new(),
            cookies: std::sync::Mutex::new(CookieJar::default()),
        });

        let builder = TestWebSocketBuilder::new(shared, "/ws")
            .query("a", "1")
            .query("b", "two");

        assert_eq!(
            builder.query,
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "two".to_owned()),
            ]
        );
    }
}
