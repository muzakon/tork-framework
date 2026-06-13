//! The HTTP server: a Hyper accept loop with graceful shutdown.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::service::Service;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
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

/// A listening socket the accept loop can drive, abstracting over TCP and Unix.
///
/// `accept_io` yields a connection stream and an optional peer address (Unix-domain
/// connections have no `SocketAddr`).
pub(crate) trait IncomingListener {
    type Io: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn accept_io(
        &self,
    ) -> impl Future<Output = std::io::Result<(Self::Io, Option<std::net::SocketAddr>)>> + Send;
}

impl IncomingListener for TcpListener {
    type Io = tokio::net::TcpStream;

    async fn accept_io(&self) -> std::io::Result<(Self::Io, Option<std::net::SocketAddr>)> {
        let (stream, peer) = self.accept().await?;
        Ok((stream, Some(peer)))
    }
}

#[cfg(unix)]
impl IncomingListener for tokio::net::UnixListener {
    type Io = tokio::net::UnixStream;

    async fn accept_io(&self) -> std::io::Result<(Self::Io, Option<std::net::SocketAddr>)> {
        let (stream, _addr) = self.accept().await?;
        Ok((stream, None))
    }
}

/// Binds a TCP listener for `addr`, optionally setting `SO_REUSEPORT`.
///
/// Without `reuse_port` this is a plain `TcpListener::bind`. With it, the socket is
/// built by hand so `SO_REUSEPORT` (Unix) can be set before binding, letting several
/// processes share the address.
pub(crate) async fn bind_tcp_listener(addr: &str, reuse_port: bool) -> std::io::Result<TcpListener> {
    if !reuse_port {
        return TcpListener::bind(addr).await;
    }

    let resolved = tokio::net::lookup_host(addr).await?.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address resolved")
    })?;
    let domain = if resolved.is_ipv6() {
        socket2::Domain::IPV6
    } else {
        socket2::Domain::IPV4
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&resolved.into())?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

/// Runs the accept loop on `listener`, stopping when `shutdown` resolves.
///
/// The application lifecycle around this loop (startup, bind, readiness, drain,
/// shutdown) lives in [`App::serve`](crate::App::serve).
pub(crate) async fn run_with_shutdown<S, L>(app: Arc<AppInner>, listener: L, shutdown: S)
where
    S: Future<Output = ()>,
    L: IncomingListener,
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

/// Accept loop for plain (non-TLS) connections.
async fn accept_plain<S, L>(
    app: &Arc<AppInner>,
    listener: &L,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shutdown: &mut Pin<&mut S>,
) where
    S: Future<Output = ()>,
    L: IncomingListener,
{
    loop {
        tokio::select! {
            accepted = listener.accept_io() => {
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
async fn accept_tls<S, L>(
    app: &Arc<AppInner>,
    listener: &L,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shutdown: &mut Pin<&mut S>,
    acceptor: tokio_rustls::TlsAcceptor,
) where
    S: Future<Output = ()>,
    L: IncomingListener,
{
    type Handshaked<Io> = (tokio_rustls::server::TlsStream<Io>, Option<std::net::SocketAddr>);
    let (handshake_tx, mut handshake_rx) =
        tokio::sync::mpsc::channel::<Handshaked<L::Io>>(256);

    loop {
        tokio::select! {
            accepted = listener.accept_io() => {
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
    accepted: std::io::Result<(S, Option<std::net::SocketAddr>)>,
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

    // Wrap the stream in an idle-timeout guard when one is configured, so a
    // connection with no read/write activity is dropped instead of held open.
    match app.idle_timeout() {
        Some(idle) => serve_io(app, builder, graceful, IdleTimeoutStream::new(stream, idle), peer),
        None => serve_io(app, builder, graceful, stream, peer),
    }

    true
}

/// Serves one connection: hands the (possibly wrapped) stream to hyper, tracks it
/// for graceful drain, and drives it on a spawned task.
fn serve_io<IO>(
    app: Arc<AppInner>,
    builder: &auto::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    stream: IO,
    peer: Option<std::net::SocketAddr>,
) where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let service = match peer {
        Some(peer) => TorkService::with_peer_addr(app, peer),
        None => TorkService::new(app),
    };
    let connection = builder.serve_connection_with_upgrades(io, service);
    let watched = graceful.watch(connection.into_owned());

    tokio::spawn(async move {
        // Connection-level errors are already terminal for that
        // connection; nothing actionable remains here.
        let _ = watched.await;
    });
}

/// A stream wrapper that ends the connection after `idle` with no read or write
/// activity. The struct is `Unpin` (the timer is boxed, the inner stream is
/// `Unpin`), so the poll methods project without any `unsafe`.
struct IdleTimeoutStream<S> {
    inner: S,
    timer: Pin<Box<tokio::time::Sleep>>,
    idle: Duration,
}

impl<S> IdleTimeoutStream<S> {
    fn new(inner: S, idle: Duration) -> Self {
        Self {
            inner,
            timer: Box::pin(tokio::time::sleep(idle)),
            idle,
        }
    }

    /// Pushes the idle deadline forward after activity.
    fn touch(&mut self) {
        self.timer
            .as_mut()
            .reset(tokio::time::Instant::now() + self.idle);
    }

    /// Returns `true` once the idle deadline has passed (and registers a wake-up).
    fn idle_expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.timer.as_mut().poll(cx).is_ready()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for IdleTimeoutStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.idle_expired(cx) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connection idle timeout",
            )));
        }
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() != before {
                    this.touch();
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for IdleTimeoutStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                this.touch();
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
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
                Ok((stream, Some("127.0.0.1:0".parse().unwrap())))
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

    #[cfg(unix)]
    #[tokio::test]
    async fn reuse_port_allows_two_listeners_on_the_same_port() {
        // With SO_REUSEPORT, a second listener can bind the port the first holds.
        let first = bind_tcp_listener("127.0.0.1:0", true).await.unwrap();
        let addr = first.local_addr().unwrap();
        let second = bind_tcp_listener(&addr.to_string(), true).await.unwrap();
        assert_eq!(second.local_addr().unwrap().port(), addr.port());
    }
}
