//! The in-process test client and its builder.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::header::HOST;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::TestOverrides;
use super::cookie::CookieJar;
use super::recorder::LogRecorder;
use super::request::{PendingBody, TestRequestBuilder};
use super::response::TestResponse;
use super::websocket::TestWebSocketBuilder;
use crate::app::{App, AppInner, TestApp};
use crate::body::{BoxError, ReqBody, box_body};
use crate::error::{Error, Result};
use crate::state::StateMap;

/// A boxed streaming response body, used to read Server-Sent Events incrementally.
pub(crate) type StreamingBody =
    Pin<Box<dyn http_body::Body<Data = Bytes, Error = BoxError> + Send>>;

/// A closure that registers one override resource into the state map.
type ResourceRegister = Box<dyn FnOnce(&mut StateMap)>;

/// How a built request reaches the application.
///
/// In-process is the default (no network). A real-port variant is added later for
/// end-to-end tests.
pub(crate) enum Transport {
    /// Drive the application directly, in process.
    InProcess(Arc<AppInner>),
    /// Talk to a real server bound to this address over a loopback socket.
    RealPort(SocketAddr),
}

impl Transport {
    /// The bound address, for a real-port transport.
    pub(crate) fn address(&self) -> Option<SocketAddr> {
        match self {
            Transport::InProcess(_) => None,
            Transport::RealPort(addr) => Some(*addr),
        }
    }

    /// Executes a request and returns the status, headers, and full body.
    pub(crate) async fn execute(
        &self,
        request: http::Request<ReqBody>,
    ) -> Result<(StatusCode, HeaderMap, Bytes)> {
        match self {
            Transport::InProcess(app) => {
                let response = app.clone().handle(request).await;
                let (parts, body) = response.into_parts();
                let bytes = collect_body(body).await?;
                Ok((parts.status, parts.headers, bytes))
            }
            Transport::RealPort(addr) => {
                let response = send_over_socket(*addr, request).await?;
                let (parts, body) = response.into_parts();
                let bytes = collect_body(body).await?;
                Ok((parts.status, parts.headers, bytes))
            }
        }
    }

    /// Executes a request and returns the streaming body unread, for SSE.
    pub(crate) async fn execute_streaming(
        &self,
        request: http::Request<ReqBody>,
    ) -> Result<(StatusCode, HeaderMap, StreamingBody)> {
        match self {
            Transport::InProcess(app) => {
                let response = app.clone().handle(request).await;
                let (parts, body) = response.into_parts();
                Ok((parts.status, parts.headers, Box::pin(body)))
            }
            Transport::RealPort(addr) => {
                let response = send_over_socket(*addr, request).await?;
                let (parts, body) = response.into_parts();
                let boxed: StreamingBody = Box::pin(body.map_err(|error| Box::new(error) as BoxError));
                Ok((parts.status, parts.headers, boxed))
            }
        }
    }
}

/// Collects a response body into a single buffer.
async fn collect_body<B>(body: B) -> Result<Bytes>
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::fmt::Display,
{
    let collected = body
        .collect()
        .await
        .map_err(|error| Error::internal(format!("failed to read response body: {error}")))?;
    Ok(collected.to_bytes())
}

/// Sends a request to a real server over a fresh loopback connection.
async fn send_over_socket(
    addr: SocketAddr,
    mut request: http::Request<ReqBody>,
) -> Result<http::Response<hyper::body::Incoming>> {
    // HTTP/1.1 requires a Host header; add one derived from the address if absent.
    if !request.headers().contains_key(HOST) {
        if let Ok(value) = HeaderValue::from_str(&addr.to_string()) {
            request.headers_mut().insert(HOST, value);
        }
    }

    let stream = TcpStream::connect(addr)
        .await
        .map_err(|error| Error::internal(format!("failed to connect to {addr}: {error}")))?;
    let io = TokioIo::new(stream);
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|error| Error::internal(format!("client handshake failed: {error}")))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender
        .send_request(request)
        .await
        .map_err(|error| Error::internal(format!("request failed: {error}")))
}

/// State shared between the client and its request builders.
pub(crate) struct Shared {
    pub(crate) transport: Transport,
    pub(crate) default_headers: HeaderMap,
    pub(crate) cookies: Mutex<CookieJar>,
}

impl Shared {
    /// Builds, sends, and captures a request, updating the cookie jar.
    pub(crate) async fn send(
        &self,
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        headers: Vec<(HeaderName, HeaderValue)>,
        body: PendingBody,
    ) -> Result<TestResponse> {
        let request = self.build_request(method, &path, &query, headers, body)?;
        let (status, headers, bytes) = self.transport.execute(request).await?;
        self.cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .store(&headers);
        Ok(TestResponse {
            status,
            headers,
            body: bytes,
        })
    }

    /// Opens a Server-Sent Events stream, returning a reader over the response
    /// body. The body is not collected, so events are read as they arrive.
    pub(crate) async fn open_sse(
        &self,
        method: Method,
        path: String,
        query: Vec<(String, String)>,
        headers: Vec<(HeaderName, HeaderValue)>,
    ) -> Result<super::sse::TestSseStream> {
        let request = self.build_request(method, &path, &query, headers, PendingBody::default())?;
        let (_status, headers, body) = self.transport.execute_streaming(request).await?;
        self.cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .store(&headers);
        Ok(super::sse::TestSseStream::new(body))
    }

    /// Assembles an `http::Request`, merging default headers, the cookie jar, and
    /// the body's content type.
    pub(crate) fn build_request(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        headers: Vec<(HeaderName, HeaderValue)>,
        body: PendingBody,
    ) -> Result<http::Request<ReqBody>> {
        let uri = if query.is_empty() {
            path.to_owned()
        } else {
            let encoded = serde_urlencoded::to_string(query)
                .map_err(|_| Error::internal("failed to encode query parameters"))?;
            format!("{path}?{encoded}")
        };

        let mut request = http::Request::new(box_body(Full::new(body.bytes)));
        *request.method_mut() = method;
        *request.uri_mut() = uri
            .parse()
            .map_err(|_| Error::bad_request(format!("invalid request URI: {uri}")))?;

        let map = request.headers_mut();
        for (name, value) in self.default_headers.iter() {
            map.insert(name, value.clone());
        }
        for (name, value) in headers {
            map.insert(name, value);
        }
        self.cookies
            .lock()
            .expect("cookie jar mutex poisoned")
            .apply(map);
        if let Some(content_type) = body.content_type {
            map.insert(super::request::CONTENT_TYPE_HEADER, content_type);
        }

        Ok(request)
    }
}

/// An in-process client for exercising an application in tests.
///
/// Build it from a [`TestApp`] with [`TestClient::new`], or configure overrides
/// through [`TestClient::builder`]. Requests run straight through the request
/// pipeline with no network. Call [`shutdown`](TestClient::shutdown) when finished
/// to run the lifespan teardown.
pub struct TestClient {
    shared: Arc<Shared>,
    teardown: Teardown,
    // Routes this client's logs to a recorder for the test's lifetime. Held to
    // keep the thread-local subscriber active; `None` without a recorder.
    _log_guard: Option<tracing::subscriber::DefaultGuard>,
}

/// How a client tears down when finished.
enum Teardown {
    /// An in-process app: run its lifespan shutdown.
    InProcess(Box<TestApp>),
    /// A real server task: signal shutdown and wait for it to drain.
    RealPort {
        shutdown: Option<oneshot::Sender<()>>,
        handle: JoinHandle<()>,
    },
}

impl TestClient {
    /// Builds a client from an already-built [`TestApp`].
    pub async fn new(app: TestApp) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(Shared {
                transport: Transport::InProcess(app.inner.clone()),
                default_headers: HeaderMap::new(),
                cookies: Mutex::new(CookieJar::default()),
            }),
            teardown: Teardown::InProcess(Box::new(app)),
            _log_guard: None,
        })
    }

    /// Starts a builder for a client with resource and dependency overrides.
    pub fn builder(app: App) -> TestClientBuilder {
        TestClientBuilder::new(app)
    }

    /// Starts a real-server end-to-end client backed by a loopback socket.
    pub fn serve(app: App) -> ServeBuilder {
        ServeBuilder { app }
    }

    /// The bound address, for a real-port client (`None` when in process).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.shared.transport.address()
    }

    /// Starts a WebSocket connection.
    pub fn websocket(&self, path: &str) -> TestWebSocketBuilder {
        TestWebSocketBuilder::new(self.shared.clone(), path)
    }

    /// Starts a `GET` request.
    pub fn get(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::GET, path)
    }

    /// Starts a `POST` request.
    pub fn post(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::POST, path)
    }

    /// Starts a `PUT` request.
    pub fn put(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::PUT, path)
    }

    /// Starts a `PATCH` request.
    pub fn patch(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::PATCH, path)
    }

    /// Starts a `DELETE` request.
    pub fn delete(&self, path: &str) -> TestRequestBuilder {
        TestRequestBuilder::new(self.shared.clone(), Method::DELETE, path)
    }

    /// Tears the client down: runs the lifespan shutdown for an in-process client,
    /// or stops the server task for a real-port client.
    pub async fn shutdown(self) -> Result<()> {
        match self.teardown {
            Teardown::InProcess(app) => app.shutdown().await,
            Teardown::RealPort { shutdown, handle } => {
                if let Some(sender) = shutdown {
                    let _ = sender.send(());
                }
                let _ = handle.await;
                Ok(())
            }
        }
    }
}

/// Builds a real-server end-to-end client.
///
/// `bind_random_port` runs the full application lifecycle (including lifespan
/// startup) on a loopback socket bound to an ephemeral port, then returns a client
/// that talks to it over real connections.
pub struct ServeBuilder {
    app: App,
}

impl ServeBuilder {
    /// Binds the server to `127.0.0.1:0` and returns a connected client.
    pub async fn bind_random_port(self) -> Result<TestClient> {
        let (addr_tx, addr_rx) = oneshot::channel::<Result<SocketAddr>>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(addr_tx)));
        let ready_sender = sender.clone();

        let app = self.app.on_ready(move |ctx| {
            let sender = ready_sender.clone();
            async move {
                if let Some(tx) = sender.lock().expect("address sender mutex poisoned").take() {
                    let _ = tx.send(Ok(ctx.addr()));
                }
                Ok(())
            }
        });

        let sender = sender.clone();
        let handle = tokio::spawn(async move {
            let result = app
                .serve_with_shutdown("127.0.0.1:0", async move {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let (Err(error), Some(tx)) = (
                result,
                sender.lock().expect("address sender mutex poisoned").take(),
            ) {
                let _ = tx.send(Err(error));
            }
        });

        let addr = addr_rx
            .await
            .map_err(|_| Error::internal("the test server failed to start"))??;

        Ok(TestClient {
            shared: Arc::new(Shared {
                transport: Transport::RealPort(addr),
                default_headers: HeaderMap::new(),
                cookies: Mutex::new(CookieJar::default()),
            }),
            teardown: Teardown::RealPort {
                shutdown: Some(shutdown_tx),
                handle,
            },
            _log_guard: None,
        })
    }
}

/// A builder for a [`TestClient`] with resource and dependency overrides, default
/// headers, and seeded cookies.
pub struct TestClientBuilder {
    app: App,
    resources: Vec<ResourceRegister>,
    overrides: TestOverrides,
    default_headers: HeaderMap,
    cookies: CookieJar,
    recorder: Option<LogRecorder>,
}

impl TestClientBuilder {
    fn new(app: App) -> Self {
        Self {
            app,
            resources: Vec::new(),
            overrides: TestOverrides::default(),
            default_headers: HeaderMap::new(),
            cookies: CookieJar::default(),
            recorder: None,
        }
    }

    /// Captures this client's logs into `recorder` for the test's lifetime.
    ///
    /// Works with the default current-thread test runtime: the recorder is set as
    /// the thread's subscriber while the client is alive.
    pub fn logger(mut self, recorder: LogRecorder) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Registers (or overrides) a resource by type, applied after startup so it
    /// wins over a value the lifespan registered.
    pub fn resource<S: Send + Sync + 'static>(mut self, value: S) -> Self {
        self.resources
            .push(Box::new(move |state| state.insert(value)));
        self
    }

    /// Overrides an injected dependency with a pre-built value, cloned per request.
    pub fn override_dependency<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.overrides.insert::<T, _>(move || value.clone());
        self
    }

    /// Overrides an injected dependency with a factory invoked per request.
    pub fn override_dependency_with<T, F>(mut self, factory: F) -> Self
    where
        T: Send + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.overrides.insert::<T, F>(factory);
        self
    }

    /// Sets a default header sent with every request.
    pub fn default_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            self.default_headers.insert(name, value);
        }
        self
    }

    /// Seeds a cookie sent with every request.
    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.set(name, value);
        self
    }

    /// Builds the client, running startup and applying the overrides.
    pub async fn build(self) -> Result<TestClient> {
        let resources = self.resources;
        let overrides = self.overrides;
        let default_headers = self.default_headers;
        let cookies = self.cookies;
        let recorder = self.recorder;

        let app = self
            .app
            .build_test_with(move |state| {
                for register in resources {
                    register(state);
                }
                if !overrides.is_empty() {
                    state.insert(overrides);
                }
            })
            .await?;

        // Route this client's logs to the recorder via a thread-local subscriber.
        let log_guard = recorder.map(|recorder| {
            use tracing_subscriber::layer::SubscriberExt;
            let subscriber = tracing_subscriber::registry().with(recorder);
            tracing::subscriber::set_default(subscriber)
        });

        Ok(TestClient {
            shared: Arc::new(Shared {
                transport: Transport::InProcess(app.inner.clone()),
                default_headers,
                cookies: Mutex::new(cookies),
            }),
            teardown: Teardown::InProcess(Box::new(app)),
            _log_guard: log_guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::body::{BoxError, RespBody};
    use crate::response::Response as TorkResponse;
    use crate::router::{BoxFuture, HandlerFn, Route, Router};
    use bytes::Bytes;
    use http::header::{CONTENT_TYPE, COOKIE};
    use http_body::Frame;
    use http_body_util::{BodyExt, StreamBody};
    use futures_util::stream;
    use std::sync::Arc;

    fn json_handler() -> HandlerFn {
        Arc::new(|_ctx: crate::extract::RequestContext| -> BoxFuture<'static, crate::Result<TorkResponse>> {
            Box::pin(async { Ok(crate::json_response(crate::StatusCode::OK, &serde_json::json!({ "ok": true }))) })
        })
    }

    fn stream_handler() -> HandlerFn {
        Arc::new(|_ctx: crate::extract::RequestContext| -> BoxFuture<'static, crate::Result<TorkResponse>> {
            Box::pin(async {
                let frames = stream::iter(vec![
                    Ok::<_, BoxError>(Frame::data(Bytes::from_static(b"one"))),
                    Ok(Frame::data(Bytes::from_static(b"two"))),
                ]);
                let body = RespBody::stream(StreamBody::new(frames));
                let mut response = TorkResponse::new(body);
                *response.status_mut() = crate::StatusCode::OK;
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    http::HeaderValue::from_static("text/event-stream"),
                );
                Ok(response)
            })
        })
    }

    fn shared() -> Shared {
        let mut default_headers = HeaderMap::new();
        default_headers.insert("x-default", HeaderValue::from_static("on"));
        let mut cookies = CookieJar::default();
        cookies.set("sid", "abc");
        Shared {
            transport: Transport::InProcess(Arc::new(App::new().build().unwrap())),
            default_headers,
            cookies: Mutex::new(cookies),
        }
    }

    #[test]
    fn build_request_merges_defaults_headers_cookies_and_content_type() {
        let request = shared()
            .build_request(
                Method::POST,
                "/items",
                &[("q".to_owned(), "hello world".to_owned())],
                vec![(
                    HeaderName::from_static("x-custom"),
                    HeaderValue::from_static("yes"),
                )],
                PendingBody {
                    content_type: Some(HeaderValue::from_static("application/json")),
                    bytes: Bytes::from_static(b"{}"),
                },
            )
            .unwrap();

        assert_eq!(request.uri(), "/items?q=hello+world");
        assert_eq!(request.headers()["x-default"], "on");
        assert_eq!(request.headers()["x-custom"], "yes");
        assert_eq!(request.headers()[COOKIE], "sid=abc");
        assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
    }

    #[test]
    fn build_request_rejects_invalid_uri() {
        let error = shared()
            .build_request(
                Method::GET,
                "http://[",
                &[],
                Vec::new(),
                PendingBody::default(),
            )
            .unwrap_err();

        assert_eq!(error.kind(), crate::error::ErrorKind::BadRequest);
        assert!(error.message().starts_with("invalid request URI:"));
    }

    #[tokio::test]
    async fn real_port_transport_exercises_execute_and_execute_streaming() {
        let app = App::new()
            .include_router(
                Router::new()
                    .route(Route::new(Method::GET, "/json", json_handler()))
                    .route(Route::new(Method::GET, "/stream", stream_handler())),
            )
            ;
        let client = TestClient::serve(app).bind_random_port().await.unwrap();

        assert!(client.local_addr().is_some());
        assert!(client.shared.transport.address().is_some());

        let request = client
            .shared
            .build_request(Method::GET, "/json", &[], Vec::new(), PendingBody::default())
            .unwrap();
        let (status, headers, bytes) = client.shared.transport.execute(request).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "application/json");
        assert!(bytes.contains(&b'o'));

        let request = client
            .shared
            .build_request(Method::GET, "/stream", &[], Vec::new(), PendingBody::default())
            .unwrap();
        let (status, headers, mut body) = client
            .shared
            .transport
            .execute_streaming(request)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CONTENT_TYPE], "text/event-stream");
        let mut saw_data = false;
        while let Some(frame) = body.frame().await {
            let frame = frame.unwrap();
            if frame.into_data().is_ok() {
                saw_data = true;
            }
        }
        assert!(saw_data);

        client.shutdown().await.unwrap();
    }
}
