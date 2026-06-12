//! The HTTP server: a Hyper accept loop with graceful shutdown.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use hyper::body::Incoming;
use hyper::service::Service;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::app::AppInner;
use crate::body::{RespBody, box_body};
use crate::constants::GRACEFUL_SHUTDOWN_TIMEOUT;

/// A Hyper [`Service`] that hands each request to the application.
///
/// The service error type is [`Infallible`]: application errors are rendered
/// into responses by [`AppInner::dispatch`], so a failing request never tears
/// down the underlying connection.
#[derive(Clone)]
pub struct TorkService {
    app: Arc<AppInner>,
}

impl TorkService {
    /// Creates a service backed by the given application core.
    pub fn new(app: Arc<AppInner>) -> Self {
        Self { app }
    }
}

impl Service<Request<Incoming>> for TorkService {
    type Response = Response<RespBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: Request<Incoming>) -> Self::Future {
        let app = self.app.clone();
        Box::pin(async move {
            // Erase the connection body into the runtime's request body type.
            let (parts, incoming) = request.into_parts();
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
    let builder = auto::Builder::new(TokioExecutor::new());
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let _ = handle_accepted_connection(app.clone(), &builder, &graceful, accepted).await;
            }
            _ = &mut shutdown => break,
        }
    }

    // Stop accepting, then drain in-flight connections within the timeout.
    drain_with_timeout(graceful.shutdown(), tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT)).await;
}

/// Resolves when the process receives an interrupt or termination signal.
pub(crate) async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
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
    let (stream, _peer) = match accepted {
        Ok(pair) => pair,
        // Transient accept errors (for example, fd exhaustion) should
        // not bring the server down; skip and keep accepting.
        Err(_) => return false,
    };

    let io = TokioIo::new(stream);
    let service = TorkService::new(app.clone());
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
    use crate::{Method, StatusCode, json_response};

    use std::sync::Arc;
    use std::future;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn serves_a_request_over_tcp() {
        let handler: HandlerFn =
            Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, crate::Result<TorkResponse>> {
                Box::pin(async {
                    Ok(json_response(StatusCode::OK, &serde_json::json!({ "pong": true })))
                })
            });
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

        assert!(response.contains("200 OK"), "unexpected response: {response}");
        assert!(response.contains("\"pong\":true"), "unexpected body: {response}");

        let _ = shutdown_tx.send(());
        let _ = server.await;
    }

    #[tokio::test]
    async fn helper_paths_cover_accept_errors_shutdown_and_signals() {
        let builder = auto::Builder::new(TokioExecutor::new());
        let app = Arc::new(App::new().build().unwrap());
        let graceful = GracefulShutdown::new();

        assert!(!handle_accepted_connection::<tokio::io::DuplexStream>(
            app.clone(),
            &builder,
            &graceful,
            Err(std::io::Error::other("accept failed"))
        )
        .await);

        let (stream, _peer) = tokio::io::duplex(16);
        assert!(handle_accepted_connection(
            app,
            &builder,
            &graceful,
            Ok((stream, "127.0.0.1:0".parse().unwrap()))
        )
        .await);

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
