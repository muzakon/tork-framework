//! The HTTP server: a Hyper accept loop with graceful shutdown.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use hyper::body::Incoming;
use hyper::service::Service;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::app::AppInner;
use crate::body::{box_body, RespBody};
use crate::constants::GRACEFUL_SHUTDOWN_TIMEOUT;
use crate::extract::RequestPeerAddr;

/// Maximum time allowed for a TLS handshake to complete before the pending
/// connection is dropped, so a stalled client cannot hold a slot.
#[cfg(feature = "tls")]
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP/2 connection tuning applied to every served connection.
///
/// Each unset field keeps hyper's default. Configure with
/// [`App::http2`](crate::App::http2).
#[derive(Clone, Default)]
pub struct Http2Config {
    max_concurrent_streams: Option<u32>,
    keep_alive_interval: Option<Duration>,
    keep_alive_timeout: Option<Duration>,
    initial_stream_window_size: Option<u32>,
    initial_connection_window_size: Option<u32>,
    max_frame_size: Option<u32>,
    max_header_list_size: Option<u32>,
}

impl Http2Config {
    /// Creates an empty config (every limit at hyper's default).
    pub fn new() -> Self {
        Self::default()
    }

    /// Caps the number of concurrent streams a peer may open on one connection.
    pub fn max_concurrent_streams(mut self, max: u32) -> Self {
        self.max_concurrent_streams = Some(max);
        self
    }

    /// Sends an HTTP/2 PING on an idle connection at this interval (keep-alive).
    pub fn keep_alive_interval(mut self, interval: Duration) -> Self {
        self.keep_alive_interval = Some(interval);
        self
    }

    /// Closes the connection if a keep-alive PING is not answered within this time.
    pub fn keep_alive_timeout(mut self, timeout: Duration) -> Self {
        self.keep_alive_timeout = Some(timeout);
        self
    }

    /// Sets the initial per-stream flow-control window, in bytes.
    pub fn initial_stream_window_size(mut self, bytes: u32) -> Self {
        self.initial_stream_window_size = Some(bytes);
        self
    }

    /// Sets the initial connection-level flow-control window, in bytes.
    pub fn initial_connection_window_size(mut self, bytes: u32) -> Self {
        self.initial_connection_window_size = Some(bytes);
        self
    }

    /// Sets the largest frame payload the server will accept, in bytes.
    pub fn max_frame_size(mut self, bytes: u32) -> Self {
        self.max_frame_size = Some(bytes);
        self
    }

    /// Sets the maximum size of the decoded request header block, in bytes.
    pub fn max_header_list_size(mut self, bytes: u32) -> Self {
        self.max_header_list_size = Some(bytes);
        self
    }
}

/// HTTP/1 connection tuning applied to every served connection.
///
/// Each unset field keeps hyper's default. Configure with
/// [`App::http1`](crate::App::http1).
#[derive(Clone, Default)]
pub struct Http1Config {
    keep_alive: Option<bool>,
    max_headers: Option<usize>,
}

impl Http1Config {
    /// Creates an empty config (every setting at hyper's default).
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables HTTP/1 keep-alive (persistent connections).
    pub fn keep_alive(mut self, enabled: bool) -> Self {
        self.keep_alive = Some(enabled);
        self
    }

    /// Sets the maximum number of request headers accepted.
    pub fn max_headers(mut self, max: usize) -> Self {
        self.max_headers = Some(max);
        self
    }
}

/// Applies the app's HTTP/1 + HTTP/2 tuning onto the connection builder.
fn configure_builder(builder: &mut auto::Builder<TokioExecutor>, app: &AppInner) {
    {
        let mut h1 = builder.http1();
        h1.timer(TokioTimer::new());
        if let Some(timeout) = app.header_read_timeout() {
            h1.header_read_timeout(timeout);
        }
        if let Some(config) = app.http1_config() {
            if let Some(enabled) = config.keep_alive {
                h1.keep_alive(enabled);
            }
            if let Some(max) = config.max_headers {
                h1.max_headers(max);
            }
        }
    }
    {
        let mut h2 = builder.http2();
        h2.timer(TokioTimer::new());
        if let Some(config) = app.http2_config() {
            if let Some(max) = config.max_concurrent_streams {
                h2.max_concurrent_streams(max);
            }
            if let Some(interval) = config.keep_alive_interval {
                h2.keep_alive_interval(interval);
            }
            if let Some(timeout) = config.keep_alive_timeout {
                h2.keep_alive_timeout(timeout);
            }
            if let Some(bytes) = config.initial_stream_window_size {
                h2.initial_stream_window_size(bytes);
            }
            if let Some(bytes) = config.initial_connection_window_size {
                h2.initial_connection_window_size(bytes);
            }
            if let Some(bytes) = config.max_frame_size {
                h2.max_frame_size(bytes);
            }
            if let Some(bytes) = config.max_header_list_size {
                h2.max_header_list_size(bytes);
            }
        }
    }
}

/// A Hyper [`Service`] that hands each request to the application.
///
/// The service error type is [`Infallible`]: application errors are rendered
/// into responses by [`AppInner::dispatch`], so a failing request never tears
/// down the underlying connection.
#[derive(Clone)]
pub struct TorkService {
    app: Arc<AppInner>,
    peer_addr: Option<std::net::SocketAddr>,
}

impl TorkService {
    /// Creates a service backed by the given application core.
    pub fn new(app: Arc<AppInner>) -> Self {
        Self {
            app,
            peer_addr: None,
        }
    }

    pub(crate) fn with_peer_addr(app: Arc<AppInner>, peer_addr: std::net::SocketAddr) -> Self {
        Self {
            app,
            peer_addr: Some(peer_addr),
        }
    }
}

impl Service<Request<Incoming>> for TorkService {
    type Response = Response<RespBody>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let app = self.app.clone();
        let peer_addr = self.peer_addr;
        Box::pin(async move {
            // Erase the connection body into the runtime's request body type.
            let (mut parts, incoming) = request.into_parts();
            if let Some(peer_addr) = peer_addr {
                parts.extensions.insert(RequestPeerAddr(peer_addr));
            }
            let request = Request::from_parts(parts, box_body(incoming));
            Ok(app.handle(request).await)
        })
    }
}

/// Runs the accept loop on `listener`, stopping when `shutdown` resolves.
///
/// The application lifecycle around this loop (startup, bind, readiness, drain,
/// shutdown) lives in [`App::serve`](crate::App::serve).
pub(crate) async fn run_with_shutdown<S>(app: Arc<AppInner>, listener: TcpListener, shutdown: S)
where
    S: Future<Output = ()>,
{
    let mut builder = auto::Builder::new(TokioExecutor::new());
    // Wire the per-connection timers and the configured HTTP/1 + HTTP/2 tuning
    // (including the slowloris-bounding header-read timeout) onto the builder.
    configure_builder(&mut builder, &app);
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);

    // With TLS, terminate each connection through the rustls acceptor before
    // handing it to hyper; otherwise serve the plain TCP stream directly.
    #[cfg(feature = "tls")]
    if let Some(acceptor) = app.tls_acceptor().cloned() {
        accept_tls(&app, &listener, &builder, &graceful, &mut shutdown, acceptor).await;
    } else {
        accept_plain(&app, &listener, &builder, &graceful, &mut shutdown).await;
    }
    #[cfg(not(feature = "tls"))]
    accept_plain(&app, &listener, &builder, &graceful, &mut shutdown).await;

    // Tell in-flight WebSocket connections to close cleanly. They run in spawned
    // upgrade tasks that `GracefulShutdown` does not track, so without this they
    // would simply be dropped when the runtime stops.
    app.begin_ws_shutdown();

    // Stop accepting, then drain in-flight connections within the timeout.
    drain_with_timeout(
        graceful.shutdown(),
        tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT),
    )
    .await;
}

/// Accept loop for plain TCP connections.
async fn accept_plain<S>(
    app: &Arc<AppInner>,
    listener: &TcpListener,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shutdown: &mut Pin<&mut S>,
) where
    S: Future<Output = ()>,
{
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let _ = handle_accepted_connection(app.clone(), builder, graceful, accepted).await;
            }
            _ = shutdown.as_mut() => break,
        }
    }
}

/// Accept loop for TLS connections.
///
/// Each accepted socket is handed to a spawned task that performs the rustls
/// handshake under [`TLS_HANDSHAKE_TIMEOUT`], so a slow handshake never blocks the
/// accept loop. Completed TLS streams come back over a channel and are served (and
/// tracked by `GracefulShutdown`) exactly like a plain connection.
#[cfg(feature = "tls")]
async fn accept_tls<S>(
    app: &Arc<AppInner>,
    listener: &TcpListener,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shutdown: &mut Pin<&mut S>,
    acceptor: tokio_rustls::TlsAcceptor,
) where
    S: Future<Output = ()>,
{
    type Handshaked = (
        tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        std::net::SocketAddr,
    );
    let (handshake_tx, mut handshake_rx) = tokio::sync::mpsc::channel::<Handshaked>(256);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, peer)) = accepted {
                    let acceptor = acceptor.clone();
                    let handshake_tx = handshake_tx.clone();
                    tokio::spawn(async move {
                        if let Ok(Ok(tls)) =
                            tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
                        {
                            let _ = handshake_tx.send((tls, peer)).await;
                        }
                    });
                }
            }
            Some((tls, peer)) = handshake_rx.recv() => {
                let _ = handle_accepted_connection(app.clone(), builder, graceful, Ok((tls, peer))).await;
            }
            _ = shutdown.as_mut() => break,
        }
    }
}

/// Resolves when the process receives an interrupt or termination signal.
pub(crate) async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut stream) = signal(SignalKind::terminate()) {
            stream.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    shutdown_signal_with(interrupt, terminate).await;
}

async fn handle_accepted_connection<S>(
    app: Arc<AppInner>,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    accepted: std::io::Result<(S, std::net::SocketAddr)>,
) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (stream, peer) = match accepted {
        Ok(pair) => pair,
        // Transient accept errors (for example, fd exhaustion) should
        // not bring the server down; skip and keep accepting.
        Err(_) => return false,
    };

    let io = TokioIo::new(stream);
    let service = TorkService::with_peer_addr(app.clone(), peer);
    let connection = builder.serve_connection_with_upgrades(io, service);
    let watched = graceful.watch(connection.into_owned());

    tokio::spawn(async move {
        // Connection-level errors are already terminal for that
        // connection; nothing actionable remains here.
        let _ = watched.await;
    });

    true
}

async fn drain_with_timeout<F, T>(shutdown: F, timeout: T)
where
    F: Future<Output = ()>,
    T: Future<Output = ()>,
{
    tokio::select! {
        _ = shutdown => {}
        _ = timeout => {}
    }
}

async fn shutdown_signal_with<I, T>(interrupt: I, terminate: T)
where
    I: Future<Output = ()>,
    T: Future<Output = ()>,
{
    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::extract::RequestContext;
    use crate::response::Response as TorkResponse;
    use crate::router::{BoxFuture, HandlerFn, Route, Router};
    use crate::{json_response, Method, StatusCode};

    use std::future;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn serves_a_request_over_tcp() {
        let handler: HandlerFn = Arc::new(
            |_ctx: RequestContext| -> BoxFuture<'static, crate::Result<TorkResponse>> {
                Box::pin(async {
                    Ok(json_response(
                        StatusCode::OK,
                        &serde_json::json!({ "pong": true }),
                    ))
                })
            },
        );
        let router = Router::new().route(Route::new(Method::GET, "/ping", handler));
        let app = Arc::new(App::new().include_router(router).build().unwrap());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(run_with_shutdown(app, listener, async move {
            let _ = shutdown_rx.await;
        }));

        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        assert!(
            response.contains("200 OK"),
            "unexpected response: {response}"
        );
        assert!(
            response.contains("\"pong\":true"),
            "unexpected body: {response}"
        );

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn helper_paths_cover_accept_errors_shutdown_and_signals() {
        let builder = auto::Builder::new(TokioExecutor::new());
        let app = Arc::new(App::new().build().unwrap());
        let graceful = GracefulShutdown::new();

        assert!(
            !handle_accepted_connection::<tokio::io::DuplexStream>(
                app.clone(),
                &builder,
                &graceful,
                Err(std::io::Error::other("accept failed"))
            )
            .await
        );

        let (stream, _peer) = tokio::io::duplex(16);
        assert!(
            handle_accepted_connection(
                app,
                &builder,
                &graceful,
                Ok((stream, "127.0.0.1:0".parse().unwrap()))
            )
            .await
        );

        drain_with_timeout(future::ready(()), future::pending::<()>()).await;
        drain_with_timeout(future::pending::<()>(), future::ready(())).await;

        shutdown_signal_with(future::ready(()), future::pending::<()>()).await;
        shutdown_signal_with(future::pending::<()>(), future::ready(())).await;
    }

    #[tokio::test]
    async fn tork_service_new_returns_cloneable_service() {
        let app = Arc::new(App::new().build().unwrap());
        let service = TorkService::new(app);
        // Verify the service is Clone (derived).
        let _cloned = service.clone();
    }

    #[tokio::test]
    async fn run_with_shutdown_breaks_when_shutdown_resolves_first() {
        // Build a minimal app, bind to an ephemeral port, and run the loop
        // with a shutdown future that fires immediately — no connection is
        // ever accepted, exercising the `_ = &mut shutdown => break` branch.
        let app = Arc::new(App::new().build().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        run_with_shutdown(app, listener, future::ready(())).await;
    }
}
