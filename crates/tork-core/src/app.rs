//! The application builder and its finalized, request-handling core.

use std::future::Future;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::error::{Error, Result};
use crate::lifespan::{ErasedLifespan, Lifespan, LifespanCell, LifespanContext, ReadyContext};
use crate::middleware::{Middleware, Next, Request, resolve_duplicates};
use crate::openapi::OpenApiProvider;
use crate::response::{IntoResponse, Response};
use crate::router::matcher::Matcher;
use crate::router::{BoxFuture, Router};
use crate::server::{run_with_shutdown, shutdown_signal};
use crate::state::{AppStateRef, StateMap};

/// A startup or shutdown event hook.
type Hook = Box<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A post-bind readiness hook.
type ReadyHook = Box<dyn Fn(ReadyContext) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// The application builder.
///
/// `App` collects application state, routers, and optional OpenAPI configuration,
/// then either finalizes into an [`AppInner`] via [`App::build`] or starts
/// serving via [`App::serve`](crate::App::serve).
///
/// `App` is deliberately not generic over its state type: state is stored in a
/// type-erased [`StateMap`], which is what lets router modules be defined without
/// any knowledge of the concrete state type.
pub struct App {
    state: StateMap,
    routers: Vec<Router>,
    openapi: Option<Box<dyn OpenApiProvider>>,
    middleware: Vec<Arc<dyn Middleware>>,
    lifespan: Vec<Box<dyn ErasedLifespan>>,
    on_startup: Vec<Hook>,
    on_shutdown: Vec<Hook>,
    on_ready: Vec<ReadyHook>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates an empty application.
    pub fn new() -> Self {
        Self {
            state: StateMap::new(),
            routers: Vec::new(),
            openapi: None,
            middleware: Vec::new(),
            lifespan: Vec::new(),
            on_startup: Vec::new(),
            on_shutdown: Vec::new(),
            on_ready: Vec::new(),
        }
    }

    /// Registers an application state value, retrievable via the
    /// [`State`](crate::State) extractor.
    pub fn state<S: Send + Sync + 'static>(mut self, state: S) -> Self {
        self.state.insert(state);
        self
    }

    /// Mounts a router's routes on the application.
    pub fn include_router(mut self, router: Router) -> Self {
        self.routers.push(router);
        self
    }

    /// Configures OpenAPI document generation and the documentation UI.
    pub fn openapi<P: OpenApiProvider>(mut self, provider: P) -> Self {
        self.openapi = Some(Box::new(provider));
        self
    }

    /// Registers a middleware layer.
    ///
    /// Layers run in registration order, outermost first. Some middlewares may
    /// only be registered once; see [`DuplicatePolicy`](crate::DuplicatePolicy).
    pub fn middleware<M: Middleware>(mut self, middleware: M) -> Self {
        self.middleware.push(Arc::new(middleware));
        self
    }

    /// Registers a lifespan: a resource container with typed startup/shutdown.
    ///
    /// Lifespans start in registration order and stop in reverse order. Their
    /// resources are registered for injection. Using a lifespan together with
    /// [`on_startup`](App::on_startup) or [`on_shutdown`](App::on_shutdown) is a
    /// configuration error.
    pub fn lifespan<L: Lifespan>(mut self) -> Self {
        self.lifespan.push(Box::new(LifespanCell::<L>::new()));
        self
    }

    /// Registers a startup hook (for apps that do not use a lifespan).
    pub fn on_startup<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_startup.push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Registers a shutdown hook (for apps that do not use a lifespan).
    pub fn on_shutdown<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_shutdown.push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Registers a hook that runs once the listener has bound.
    ///
    /// Allowed in both lifespan and event-hook modes.
    pub fn on_ready<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(ReadyContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.on_ready
            .push(Box::new(move |ctx| Box::pin(hook(ctx))));
        self
    }

    /// Rejects mixing a lifespan with startup/shutdown event hooks.
    fn validate_lifecycle(&self) -> Result<()> {
        if !self.lifespan.is_empty() && (!self.on_startup.is_empty() || !self.on_shutdown.is_empty())
        {
            return Err(Error::internal(
                "Cannot use `.lifespan(...)` together with `.on_startup(...)` or `.on_shutdown(...)`.\n\
                 Use either lifespan or event hooks, not both.",
            )
            .with_code("LIFECYCLE_CONFLICT"));
        }
        Ok(())
    }

    /// Finalizes the application into its request-handling core.
    ///
    /// Routers are flattened into a single route table, OpenAPI documentation
    /// routes (if configured) are appended, and a [`Matcher`] is compiled.
    ///
    /// # Errors
    ///
    /// Returns an error if the route table contains an invalid or duplicate path.
    pub fn build(self) -> Result<AppInner> {
        self.validate_lifecycle()?;

        let App {
            state,
            routers,
            openapi,
            middleware,
            ..
        } = self;

        let mut routes = Vec::new();
        for router in routers {
            routes.extend(router.into_routes());
        }

        if let Some(provider) = openapi {
            let documentation = provider.documentation_routes(&routes);
            routes.extend(documentation);
        }

        let matcher = Matcher::build(routes)?;
        let middleware = resolve_duplicates(middleware)?;

        Ok(AppInner {
            state: Arc::new(state),
            matcher,
            middleware: middleware.into(),
        })
    }

    /// Runs the application lifecycle and serves on `addr` until a shutdown signal.
    ///
    /// Listens for `SIGINT` (Ctrl-C) and, on Unix, `SIGTERM`. The lifecycle runs
    /// in order: startup (lifespans or `on_startup` hooks), bind, `on_ready`
    /// hooks, the accept loop, drain, then shutdown (lifespans in reverse, or
    /// `on_shutdown` hooks).
    ///
    /// # Errors
    ///
    /// Returns an error for a lifecycle misconfiguration, a failed startup, or a
    /// bind failure.
    pub async fn serve(self, addr: impl AsRef<str>) -> Result<()> {
        self.serve_with_shutdown(addr.as_ref(), shutdown_signal())
            .await
    }

    /// Runs the lifecycle, stopping the accept loop when `shutdown` resolves.
    ///
    /// Factored out so the lifecycle can be driven by a test-controlled signal.
    pub(crate) async fn serve_with_shutdown<S>(mut self, addr: &str, shutdown: S) -> Result<()>
    where
        S: std::future::Future<Output = ()>,
    {
        self.validate_lifecycle()
            .inspect_err(|error| eprintln!("{}", error.message()))?;

        // Startup: lifespans (in order) or event hooks.
        if !self.lifespan.is_empty() {
            for index in 0..self.lifespan.len() {
                let ctx = LifespanContext::new();
                if let Err(error) = self.lifespan[index].startup(ctx, &mut self.state).await {
                    // Roll back the lifespans that already started, in reverse.
                    for started in (0..index).rev() {
                        let _ = self.lifespan[started].shutdown().await;
                    }
                    eprintln!("{}", error.message());
                    return Err(error);
                }
            }
        } else {
            for hook in &self.on_startup {
                hook()
                    .await
                    .inspect_err(|error| eprintln!("{}", error.message()))?;
            }
        }

        let mut lifespan = std::mem::take(&mut self.lifespan);
        let on_shutdown = std::mem::take(&mut self.on_shutdown);
        let on_ready = std::mem::take(&mut self.on_ready);

        let app = Arc::new(
            self.build()
                .inspect_err(|error| eprintln!("{}", error.message()))?,
        );

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|error| Error::internal(format!("failed to bind {addr}: {error}")))?;
        let local = listener
            .local_addr()
            .map_err(|error| Error::internal(format!("failed to read local address: {error}")))?;

        for hook in &on_ready {
            hook(ReadyContext::new(local))
                .await
                .inspect_err(|error| eprintln!("{}", error.message()))?;
        }

        run_with_shutdown(app, listener, shutdown).await;

        // Shutdown: lifespans in reverse order, or event hooks. Errors are logged.
        if !lifespan.is_empty() {
            for cell in lifespan.iter_mut().rev() {
                if let Err(error) = cell.shutdown().await {
                    eprintln!("tork: shutdown failed: {}", error.message());
                }
            }
        } else {
            for hook in &on_shutdown {
                if let Err(error) = hook().await {
                    eprintln!("tork: shutdown hook failed: {}", error.message());
                }
            }
        }

        Ok(())
    }
}

/// The finalized application: shared state plus a compiled route matcher.
///
/// This is the value shared across all connections. It is cheap to clone behind
/// an `Arc` and is what the server hands each request to via
/// [`dispatch`](AppInner::dispatch).
pub struct AppInner {
    state: AppStateRef,
    matcher: Matcher,
    middleware: Arc<[Arc<dyn Middleware>]>,
}

impl AppInner {
    /// Returns the shared application state.
    pub fn state(&self) -> &AppStateRef {
        &self.state
    }

    /// Returns the compiled route matcher.
    pub fn matcher(&self) -> &Matcher {
        &self.matcher
    }

    /// Runs the middleware chain and dispatches the request.
    ///
    /// This is the entrypoint the server calls per request. The middleware chain
    /// wraps [`dispatch`](AppInner::dispatch); a middleware error is rendered
    /// into a response so the connection is never torn down.
    pub async fn handle(self: Arc<Self>, request: Request) -> Response {
        let next = Next::new(self.clone(), self.middleware.clone());
        match next.run(request).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Resources;
    use std::sync::atomic::{AtomicBool, Ordering};

    static STARTED: AtomicBool = AtomicBool::new(false);
    static STOPPED: AtomicBool = AtomicBool::new(false);

    #[derive(Clone)]
    struct Boot;

    impl Resources for Boot {
        fn register(&self, _registry: &mut StateMap) {}
    }

    impl Lifespan for Boot {
        async fn startup(_ctx: LifespanContext) -> Result<Self> {
            STARTED.store(true, Ordering::SeqCst);
            Ok(Boot)
        }

        async fn shutdown(self) -> Result<()> {
            STOPPED.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn serve_runs_startup_then_shutdown() {
        // An immediately-ready shutdown future stops the accept loop at once.
        App::new()
            .lifespan::<Boot>()
            .serve_with_shutdown("127.0.0.1:0", async {})
            .await
            .unwrap();

        assert!(STARTED.load(Ordering::SeqCst), "startup should have run");
        assert!(STOPPED.load(Ordering::SeqCst), "shutdown should have run");
    }

    #[test]
    fn lifespan_with_event_hooks_is_a_conflict() {
        let error = App::new()
            .lifespan::<Boot>()
            .on_startup(|| async { Ok(()) })
            .build()
            .err()
            .expect("lifespan plus on_startup should conflict");

        assert_eq!(error.code(), "LIFECYCLE_CONFLICT");
        assert!(
            error.message().contains("Use either lifespan or event hooks"),
            "message: {}",
            error.message()
        );
    }
}
