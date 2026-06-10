//! The application builder and its finalized, request-handling core.

use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::{HeaderMap, Method, StatusCode, Uri};
use tokio::net::TcpListener;

use crate::error::{Error, Result};
use crate::hooks::{
    ErrorContext, ErrorEvent, PanicEvent, RequestEvent, RequestInfo, ResponseEvent,
    ValidationErrorEvent,
};
use crate::lifespan::{ErasedLifespan, Lifespan, LifespanCell, LifespanContext, ReadyContext};
use crate::middleware::{Middleware, Next, Request, resolve_duplicates};
use crate::openapi::OpenApiProvider;
use crate::response::{IntoResponse, Response};
use crate::router::matcher::Matcher;
use crate::router::{
    BoxFuture, Route, Router, SharedErrorHook, SharedRequestHook, SharedResponseHook,
    SharedValidationErrorHook,
};
use crate::server::{run_with_shutdown, shutdown_signal};
use crate::state::{AppStateRef, StateMap};

/// A startup or shutdown event hook.
type Hook = Box<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A post-bind readiness hook.
type ReadyHook = Box<dyn Fn(ReadyContext) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// An observe-only hook fired when a request arrives.
type RequestHook = Box<dyn Fn(RequestEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// An observe-only hook fired when a response is ready.
type ResponseHook = Box<dyn Fn(ResponseEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// An observe-only hook fired for a non-validation error.
type ErrorHook = Box<dyn Fn(ErrorEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// An observe-only hook fired for a request-body validation failure.
type ValidationErrorHook =
    Box<dyn Fn(ValidationErrorEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// An observe-only hook fired when a handler panic is caught.
type PanicHook = Box<dyn Fn(PanicEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// Maps a recovered typed error into a response.
type ExceptionHandlerFn =
    Box<dyn Fn(Error, ErrorContext) -> BoxFuture<'static, Response> + Send + Sync>;

/// Header consulted to correlate hook events with a request identifier.
const REQUEST_ID_HEADER: &str = "x-request-id";

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
    on_request: Vec<RequestHook>,
    on_response: Vec<ResponseHook>,
    on_error: Vec<ErrorHook>,
    on_validation_error: Vec<ValidationErrorHook>,
    on_panic: Vec<PanicHook>,
    catch_panics: bool,
    exception_handlers: HashMap<TypeId, ExceptionHandlerFn>,
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
            on_request: Vec::new(),
            on_response: Vec::new(),
            on_error: Vec::new(),
            on_validation_error: Vec::new(),
            on_panic: Vec::new(),
            catch_panics: false,
            exception_handlers: HashMap::new(),
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

    /// Registers an observe-only hook that runs when a request arrives.
    ///
    /// Hooks run in registration order, before the middleware chain, and cannot
    /// alter the response. Use them for logging, metrics, or tracing.
    pub fn on_request<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(RequestEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_request.push(Box::new(move |event| Box::pin(hook(event))));
        self
    }

    /// Registers an observe-only hook that runs once a response is ready.
    ///
    /// Hooks run in registration order, after the middleware chain, and observe
    /// the final status and elapsed time.
    pub fn on_response<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(ResponseEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_response.push(Box::new(move |event| Box::pin(hook(event))));
        self
    }

    /// Registers an observe-only hook that runs for a non-validation error.
    ///
    /// Validation failures (`422`) go to [`on_validation_error`](App::on_validation_error)
    /// instead; every other error fires this hook.
    pub fn on_error<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(ErrorEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_error.push(Box::new(move |event| Box::pin(hook(event))));
        self
    }

    /// Registers an observe-only hook that runs for a request-body validation
    /// failure (`422`).
    pub fn on_validation_error<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(ValidationErrorEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_validation_error.push(Box::new(move |event| Box::pin(hook(event))));
        self
    }

    /// Registers an observe-only hook that runs when a handler panic is caught.
    ///
    /// Has no effect unless the panic boundary is enabled with
    /// [`catch_panics`](App::catch_panics).
    pub fn on_panic<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(PanicEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_panic.push(Box::new(move |event| Box::pin(hook(event))));
        self
    }

    /// Enables the panic boundary: a panic in a handler is caught and turned into
    /// a `500` response instead of dropping the connection.
    ///
    /// Disabled by default. When enabled, a caught panic fires the
    /// [`on_panic`](App::on_panic) hooks. The boundary has no effect when the
    /// process is built with `panic = "abort"`.
    pub fn catch_panics(mut self) -> Self {
        self.catch_panics = true;
        self
    }

    /// Registers a handler that maps a typed error `E` into a response.
    ///
    /// When an error carries a source of type `E` (for example one produced by a
    /// `#[derive(AppError)]` type via `?`), the registered handler receives the
    /// recovered value and produces the response. Registering a handler for a type
    /// again replaces the previous one.
    pub fn exception_handler<E, F, Fut>(mut self, handler: F) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
        F: Fn(E, ErrorContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.exception_handlers.insert(
            TypeId::of::<E>(),
            Box::new(move |mut error, ctx| match error.take_source::<E>() {
                Some(value) => Box::pin(handler(value, ctx)),
                // The source matched by type id but could not be recovered; fall
                // back to the default rendering rather than dropping the error.
                None => Box::pin(async move { error.into_response() }),
            }),
        );
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
            on_request,
            on_response,
            on_error,
            on_validation_error,
            on_panic,
            catch_panics,
            exception_handlers,
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
            on_request: on_request.into(),
            on_response: on_response.into(),
            on_error: on_error.into(),
            on_validation_error: on_validation_error.into(),
            on_panic: on_panic.into(),
            catch_panics,
            exception_handlers: Arc::new(exception_handlers),
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
        self.serve_with_shutdown(addr, shutdown_signal()).await
    }

    /// Runs the lifecycle, stopping the accept loop when `shutdown` resolves.
    ///
    /// Like [`serve`](App::serve) but driven by a caller-supplied future instead
    /// of `SIGINT`/`SIGTERM`, for custom graceful shutdown (and for tests).
    pub async fn serve_with_shutdown<S>(mut self, addr: impl AsRef<str>, shutdown: S) -> Result<()>
    where
        S: std::future::Future<Output = ()>,
    {
        let addr = addr.as_ref();
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
    on_request: Arc<[RequestHook]>,
    on_response: Arc<[ResponseHook]>,
    on_error: Arc<[ErrorHook]>,
    on_validation_error: Arc<[ValidationErrorHook]>,
    on_panic: Arc<[PanicHook]>,
    catch_panics: bool,
    exception_handlers: Arc<HashMap<TypeId, ExceptionHandlerFn>>,
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
    /// This is the entrypoint the server calls per request. `on_request` hooks run
    /// before the chain and `on_response` hooks run after it. The middleware chain
    /// wraps [`dispatch`](AppInner::dispatch); a middleware error is rendered into a
    /// response (through the error hooks and any exception handler) so the
    /// connection is never torn down.
    pub async fn handle(self: Arc<Self>, request: Request) -> Response {
        // Build request metadata once, only if some hook or handler needs it.
        let info = self
            .needs_request_info()
            .then(|| request_info(request.method(), request.uri(), request.headers(), None));
        let start = (!self.on_response.is_empty()).then(Instant::now);

        if let Some(info) = &info {
            for hook in self.on_request.iter() {
                hook(RequestEvent::new(info.clone())).await;
            }
        }

        let next = Next::new(self.clone(), self.middleware.clone());
        let response = match next.run(request).await {
            Ok(response) => response,
            // A middleware-level error is rendered here, after the chain. No route
            // matched at this point, so only the app-global hooks apply.
            Err(error) => match &info {
                Some(info) => self.render_error(error, info, None).await,
                None => error.into_response(),
            },
        };

        if let Some(info) = &info {
            let status = response.status();
            let elapsed = start.map(|start| start.elapsed()).unwrap_or_default();
            for hook in self.on_response.iter() {
                hook(ResponseEvent::new(info.clone(), status, elapsed)).await;
            }
        }

        response
    }

    /// Reports whether any registered hook or handler needs request metadata.
    ///
    /// When nothing observes the request, metadata is never built and errors are
    /// rendered directly, keeping the hook machinery zero-cost when unused.
    pub(crate) fn needs_request_info(&self) -> bool {
        !self.on_request.is_empty()
            || !self.on_response.is_empty()
            || !self.on_error.is_empty()
            || !self.on_validation_error.is_empty()
            || !self.on_panic.is_empty()
            || !self.exception_handlers.is_empty()
    }

    /// Reports whether the panic boundary is enabled.
    pub(crate) fn catch_panics(&self) -> bool {
        self.catch_panics
    }

    /// Runs the panic hooks for a caught handler panic.
    pub(crate) async fn fire_panic(&self, info: &RequestInfo, message: &str) {
        for hook in self.on_panic.iter() {
            hook(PanicEvent::new(info.clone(), message.to_owned())).await;
        }
    }

    /// Renders an error into a response, running the error hooks and any matching
    /// exception handler first.
    ///
    /// A validation failure fires `on_validation_error`; every other error fires
    /// `on_error`. The app-global hooks run first, then the matched route's scoped
    /// hooks (when `route` is `Some`). If the error carries a typed source with a
    /// registered exception handler, that handler produces the response; otherwise
    /// the default problem-details rendering is used.
    pub(crate) async fn render_error(
        &self,
        error: Error,
        info: &RequestInfo,
        route: Option<&Route>,
    ) -> Response {
        if error.is_validation() {
            for hook in self.on_validation_error.iter() {
                hook(ValidationErrorEvent::new(info.clone(), error.details().to_vec())).await;
            }
            if let Some(route) = route {
                fire_validation_hooks(route.validation_hooks(), info, &error).await;
            }
        } else {
            for hook in self.on_error.iter() {
                hook(ErrorEvent::new(
                    info.clone(),
                    error.kind().status(),
                    error.static_code(),
                    error.message().to_owned(),
                ))
                .await;
            }
            if let Some(route) = route {
                fire_error_hooks(route.error_hooks(), info, &error).await;
            }
        }

        if let Some(type_id) = error.source_type() {
            if let Some(handler) = self.exception_handlers.get(&type_id) {
                return handler(error, ErrorContext::new(info.clone())).await;
            }
        }

        error.into_response()
    }
}

/// Fires a slice of scoped `on_request` hooks in order.
pub(crate) async fn fire_request_hooks(hooks: &[SharedRequestHook], info: &RequestInfo) {
    for hook in hooks {
        hook(RequestEvent::new(info.clone())).await;
    }
}

/// Fires a slice of scoped `on_response` hooks in reverse (innermost first).
pub(crate) async fn fire_response_hooks(
    hooks: &[SharedResponseHook],
    info: &RequestInfo,
    status: StatusCode,
    elapsed: Duration,
) {
    for hook in hooks.iter().rev() {
        hook(ResponseEvent::new(info.clone(), status, elapsed)).await;
    }
}

/// Fires a slice of scoped `on_error` hooks in order.
async fn fire_error_hooks(hooks: &[SharedErrorHook], info: &RequestInfo, error: &Error) {
    for hook in hooks {
        hook(ErrorEvent::new(
            info.clone(),
            error.kind().status(),
            error.static_code(),
            error.message().to_owned(),
        ))
        .await;
    }
}

/// Fires a slice of scoped `on_validation_error` hooks in order.
async fn fire_validation_hooks(
    hooks: &[SharedValidationErrorHook],
    info: &RequestInfo,
    error: &Error,
) {
    for hook in hooks {
        hook(ValidationErrorEvent::new(info.clone(), error.details().to_vec())).await;
    }
}

/// Builds request metadata for the hook events from a request head.
pub(crate) fn request_info(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    route: Option<String>,
) -> RequestInfo {
    let request_id = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    RequestInfo::new(method.clone(), uri.path().to_owned(), route, request_id)
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

#[cfg(test)]
mod hook_tests {
    use super::*;
    use crate::ErrorDetail;
    use crate::body::box_body;
    use crate::extract::RequestContext;
    use crate::response::empty;
    use crate::router::{HandlerFn, Route};
    use bytes::Bytes;
    use http::StatusCode;
    use http_body_util::Full;
    use std::sync::Mutex;

    type Log = Arc<Mutex<Vec<String>>>;

    fn log() -> Log {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn entries(log: &Log) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    fn request(method: Method, uri: &str) -> Request {
        http::Request::builder()
            .method(method)
            .uri(uri)
            .body(box_body(Full::new(Bytes::new())))
            .unwrap()
    }

    /// A route whose handler returns the error produced by `make`.
    fn failing_route(make: fn() -> Error) -> Router {
        let handler: HandlerFn =
            Arc::new(move |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async move { Err(make()) })
            });
        Router::new().route(Route::new(Method::GET, "/", handler))
    }

    fn ok_handler() -> HandlerFn {
        Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async { Ok(empty(StatusCode::OK)) })
        })
    }

    fn ok_route() -> Router {
        Router::new().route(Route::new(Method::GET, "/", ok_handler()))
    }

    fn panicking_route() -> Router {
        let handler: HandlerFn =
            Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async { panic!("handler boom") })
            });
        Router::new().route(Route::new(Method::GET, "/", handler))
    }

    #[tokio::test]
    async fn on_error_fires_for_a_missing_route() {
        let seen = log();
        let recorder = seen.clone();
        let app = App::new()
            .on_error(move |event| {
                let recorder = recorder.clone();
                let code = event.code().to_owned();
                async move { recorder.lock().unwrap().push(code) }
            })
            .build()
            .unwrap();

        let response = app.dispatch(request(Method::GET, "/missing")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(entries(&seen), vec!["NOT_FOUND".to_owned()]);
    }

    #[tokio::test]
    async fn validation_error_fires_only_the_validation_hook() {
        fn validation_error() -> Error {
            Error::unprocessable("invalid")
                .with_code("VALIDATION_ERROR")
                .with_details(vec![ErrorDetail::new("name", "TOO_SHORT", "too short")])
        }

        let errors = log();
        let validations = log();
        let error_rec = errors.clone();
        let validation_rec = validations.clone();

        let app = App::new()
            .include_router(failing_route(validation_error))
            .on_error(move |_event| {
                let rec = error_rec.clone();
                async move { rec.lock().unwrap().push("error".to_owned()) }
            })
            .on_validation_error(move |event| {
                let rec = validation_rec.clone();
                let fields = event.details().len();
                async move { rec.lock().unwrap().push(format!("validation:{fields}")) }
            })
            .build()
            .unwrap();

        let response = app.dispatch(request(Method::GET, "/")).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(entries(&validations), vec!["validation:1".to_owned()]);
        assert!(entries(&errors).is_empty(), "on_error must not fire for validation");
    }

    #[derive(Debug)]
    struct SampleError;
    impl std::fmt::Display for SampleError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("sample failure")
        }
    }
    impl std::error::Error for SampleError {}

    #[tokio::test]
    async fn exception_handler_replaces_the_response() {
        fn sample_error() -> Error {
            Error::internal("wrapped").with_source(SampleError)
        }

        let app = App::new()
            .include_router(failing_route(sample_error))
            .exception_handler::<SampleError, _, _>(|error, _ctx| async move {
                // The recovered value is the original typed error.
                assert_eq!(error.to_string(), "sample failure");
                empty(StatusCode::SERVICE_UNAVAILABLE)
            })
            .build()
            .unwrap();

        let response = app.dispatch(request(Method::GET, "/")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn request_hooks_run_in_registration_order() {
        let seen = log();
        let first = seen.clone();
        let second = seen.clone();

        let app = Arc::new(
            App::new()
                .include_router(ok_route())
                .on_request(move |_event| {
                    let rec = first.clone();
                    async move { rec.lock().unwrap().push("first".to_owned()) }
                })
                .on_request(move |_event| {
                    let rec = second.clone();
                    async move { rec.lock().unwrap().push("second".to_owned()) }
                })
                .build()
                .unwrap(),
        );

        let response = app.handle(request(Method::GET, "/")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(entries(&seen), vec!["first".to_owned(), "second".to_owned()]);
    }

    #[tokio::test]
    async fn catch_panics_converts_a_panic_into_a_500() {
        let seen = log();
        let recorder = seen.clone();
        let app = App::new()
            .include_router(panicking_route())
            .catch_panics()
            .on_panic(move |event| {
                let recorder = recorder.clone();
                let message = event.message().to_owned();
                async move { recorder.lock().unwrap().push(message) }
            })
            .build()
            .unwrap();

        let response = app.dispatch(request(Method::GET, "/")).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(entries(&seen), vec!["handler boom".to_owned()]);
    }

    #[tokio::test]
    #[should_panic(expected = "handler boom")]
    async fn without_catch_panics_a_panic_propagates() {
        let app = App::new().include_router(panicking_route()).build().unwrap();
        let _ = app.dispatch(request(Method::GET, "/")).await;
    }

    #[tokio::test]
    async fn scoped_on_request_fires_only_for_its_router() {
        let seen = log();
        let recorder = seen.clone();
        let scoped = Router::new()
            .route(Route::new(Method::GET, "/a", ok_handler()))
            .on_request(move |_event| {
                let recorder = recorder.clone();
                async move { recorder.lock().unwrap().push("a".to_owned()) }
            });
        let plain = Router::new().route(Route::new(Method::GET, "/b", ok_handler()));
        let app = App::new()
            .include_router(scoped)
            .include_router(plain)
            .build()
            .unwrap();

        let _ = app.dispatch(request(Method::GET, "/a")).await;
        let _ = app.dispatch(request(Method::GET, "/b")).await;
        assert_eq!(entries(&seen), vec!["a".to_owned()]);
    }

    #[tokio::test]
    async fn scoped_on_error_runs_after_the_global_hook() {
        let seen = log();
        let global = seen.clone();
        let scoped = seen.clone();

        let router = failing_route(|| Error::not_found("missing")).on_error(move |_event| {
            let scoped = scoped.clone();
            async move { scoped.lock().unwrap().push("scoped".to_owned()) }
        });
        let app = App::new()
            .on_error(move |_event| {
                let global = global.clone();
                async move { global.lock().unwrap().push("global".to_owned()) }
            })
            .include_router(router)
            .build()
            .unwrap();

        let _ = app.dispatch(request(Method::GET, "/")).await;
        assert_eq!(entries(&seen), vec!["global".to_owned(), "scoped".to_owned()]);
    }

    #[tokio::test]
    async fn scoped_on_response_hooks_fire_in_reverse() {
        let seen = log();
        let first = seen.clone();
        let second = seen.clone();
        let router = Router::new()
            .route(Route::new(Method::GET, "/", ok_handler()))
            .on_response(move |_event| {
                let first = first.clone();
                async move { first.lock().unwrap().push("first".to_owned()) }
            })
            .on_response(move |_event| {
                let second = second.clone();
                async move { second.lock().unwrap().push("second".to_owned()) }
            });
        let app = App::new().include_router(router).build().unwrap();

        let _ = app.dispatch(request(Method::GET, "/")).await;
        assert_eq!(entries(&seen), vec!["second".to_owned(), "first".to_owned()]);
    }
}
