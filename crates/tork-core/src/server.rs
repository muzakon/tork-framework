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

use crate::app::{App, AppInner};
use crate::body::{RespBody, box_body};
use crate::constants::GRACEFUL_SHUTDOWN_TIMEOUT;
use crate::error::{Error, Result};

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
            Ok(app.dispatch(request).await)
        })
    }
}

impl App {
    /// Builds the application and serves it on `addr` until a shutdown signal.
    ///
    /// Listens for `SIGINT` (Ctrl-C) and, on Unix, `SIGTERM`, then stops
    /// accepting new connections and drains in-flight ones, bounded by
    /// [`GRACEFUL_SHUTDOWN_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if the route table is invalid or the address cannot be
    /// bound.
    pub async fn serve(self, addr: impl AsRef<str>) -> Result<()> {
        let app = Arc::new(self.build()?);
        run(app, addr.as_ref()).await
    }
}

/// Binds `addr` and serves until a shutdown signal arrives.
async fn run(app: Arc<AppInner>, addr: &str) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| Error::internal(format!("failed to bind {addr}: {error}")))?;

    run_with_shutdown(app, listener, shutdown_signal()).await;
    Ok(())
}

/// Runs the accept loop on `listener`, stopping when `shutdown` resolves.
///
/// Factored out from [`run`] so the loop can be driven by a test-controlled
/// shutdown future.
async fn run_with_shutdown<S>(app: Arc<AppInner>, listener: TcpListener, shutdown: S)
where
    S: Future<Output = ()>,
{
    let builder = auto::Builder::new(TokioExecutor::new());
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _peer) = match accepted {
                    Ok(pair) => pair,
                    // Transient accept errors (for example, fd exhaustion) should
                    // not bring the server down; skip and keep accepting.
                    Err(_) => continue,
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
            }
            _ = &mut shutdown => break,
        }
    }

    // Stop accepting, then drain in-flight connections within the timeout.
    tokio::select! {
        _ = graceful.shutdown() => {}
        _ = tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT) => {}
    }
}

/// Resolves when the process receives an interrupt or termination signal.
async fn shutdown_signal() {
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

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::RequestContext;
    use crate::response::Response as TorkResponse;
    use crate::router::{BoxFuture, HandlerFn, Route, Router};
    use crate::{Method, StatusCode, json_response};

    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn serves_a_request_over_tcp() {
        let handler: HandlerFn = Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, TorkResponse> {
            Box::pin(async { json_response(StatusCode::OK, &serde_json::json!({ "pong": true })) })
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
}
