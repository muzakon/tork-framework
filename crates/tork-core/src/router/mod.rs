//! The router data model: [`Route`], [`Router`], and route metadata.
//!
//! A [`Router`] groups routes under a common path prefix and tag set. Routers
//! compose by nesting (`include`), and the whole tree is flattened into a list
//! of fully-qualified [`Route`]s when mounted on the application.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use http::{Method, StatusCode};

use crate::error::Result;
use crate::extract::RequestContext;
use crate::hooks::{ErrorEvent, RequestEvent, ResponseEvent, ValidationErrorEvent};
use crate::response::Response;

pub mod matcher;

/// A boxed, sendable future of a request handler's response.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A scoped, observe-only `on_request` hook, shared across the routes it covers.
///
/// Scoped hooks are `Arc`-shared (one set fans out to every route in a router),
/// unlike the single-owner app-global hooks on [`App`](crate::App).
pub(crate) type SharedRequestHook =
    Arc<dyn Fn(RequestEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// A scoped, observe-only `on_response` hook.
pub(crate) type SharedResponseHook =
    Arc<dyn Fn(ResponseEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// A scoped, observe-only `on_error` hook.
pub(crate) type SharedErrorHook = Arc<dyn Fn(ErrorEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// A scoped, observe-only `on_validation_error` hook.
pub(crate) type SharedValidationErrorHook =
    Arc<dyn Fn(ValidationErrorEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// Generates the four scoped-hook builder methods for a type carrying the
/// matching hook fields (`Route` and `Router`).
macro_rules! scoped_hook_builders {
    () => {
        /// Registers a scoped, observe-only `on_request` hook.
        ///
        /// Scoped hooks run in addition to the app-global ones, after routing.
        pub fn on_request<F, Fut>(mut self, hook: F) -> Self
        where
            F: Fn(RequestEvent) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = ()> + Send + 'static,
        {
            self.request_hooks
                .push(Arc::new(move |event| Box::pin(hook(event))));
            self
        }

        /// Registers a scoped, observe-only `on_response` hook.
        pub fn on_response<F, Fut>(mut self, hook: F) -> Self
        where
            F: Fn(ResponseEvent) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = ()> + Send + 'static,
        {
            self.response_hooks
                .push(Arc::new(move |event| Box::pin(hook(event))));
            self
        }

        /// Registers a scoped, observe-only `on_error` hook (non-validation errors).
        pub fn on_error<F, Fut>(mut self, hook: F) -> Self
        where
            F: Fn(ErrorEvent) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = ()> + Send + 'static,
        {
            self.error_hooks
                .push(Arc::new(move |event| Box::pin(hook(event))));
            self
        }

        /// Registers a scoped, observe-only `on_validation_error` hook.
        pub fn on_validation_error<F, Fut>(mut self, hook: F) -> Self
        where
            F: Fn(ValidationErrorEvent) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = ()> + Send + 'static,
        {
            self.validation_hooks
                .push(Arc::new(move |event| Box::pin(hook(event))));
            self
        }
    };
}

/// A type-erased request handler.
///
/// Handlers of every signature are erased to this shape at the router boundary,
/// which is what lets routers store heterogeneous handlers in one table. Exactly
/// one allocation per request is paid here; all resolution inside the handler is
/// statically dispatched.
///
/// A handler returns `Result<Response>` rather than a bare `Response` so that an
/// extractor, validation, or handler error stays a value (`Err`) until it reaches
/// the dispatch boundary, where lifecycle hooks and exception handlers can observe
/// or map it before it is rendered into a response.
pub type HandlerFn =
    Arc<dyn Fn(RequestContext) -> BoxFuture<'static, Result<Response>> + Send + Sync>;

/// Produces a JSON Schema for a type, recorded as a function pointer so that
/// [`RouteMeta`] stays `Copy`-free of any concrete type while still describing
/// request and response bodies.
pub type SchemaThunk = fn(&mut schemars::SchemaGenerator) -> schemars::Schema;

/// The encoding of a route's request body, for OpenAPI documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RequestBodyKind {
    /// `application/json`.
    #[default]
    Json,
    /// `application/x-www-form-urlencoded`.
    Form,
    /// `multipart/form-data`.
    Multipart,
}

/// A single OpenAPI security requirement: a scheme name and the scopes it
/// requires. The scopes are empty for non-OAuth2 schemes such as HTTP bearer or
/// an API key.
#[derive(Clone, Debug)]
pub struct SecurityRequirement {
    /// The name of a security scheme registered on the OpenAPI document.
    pub scheme: String,
    /// The OAuth2 scopes required, empty for bearer and API-key schemes.
    pub scopes: Vec<String>,
}

/// Introspection metadata for a route, used to build the OpenAPI document.
#[derive(Clone, Debug)]
pub struct RouteMeta {
    /// Short, human-readable summary of the operation.
    pub summary: Option<String>,
    /// Longer description of the operation.
    pub description: Option<String>,
    /// Tags used to group operations in the documentation.
    pub tags: Vec<String>,
    /// Success status code the handler returns by default.
    pub status_code: StatusCode,
    /// Name of the declared response model type, recorded for documentation.
    pub response_model: Option<&'static str>,
    /// Schema generator for the request body, if the operation accepts one.
    pub request_schema: Option<SchemaThunk>,
    /// The encoding of the request body (`application/json` by default).
    pub request_kind: RequestBodyKind,
    /// Schema generator for the success response body, if any.
    pub response_schema: Option<SchemaThunk>,
    /// Whether the response is a stream (Server-Sent Events), documented as
    /// `text/event-stream` rather than `application/json`.
    pub streaming: bool,
    /// Whether this route is a WebSocket endpoint, documented as an AsyncAPI
    /// channel rather than an HTTP operation.
    pub websocket: bool,
    /// Schema generator for the messages a WebSocket receives, if declared.
    pub ws_incoming: Option<SchemaThunk>,
    /// Schema generator for the messages a WebSocket sends, if declared.
    pub ws_outgoing: Option<SchemaThunk>,
    /// OpenAPI security requirements for this operation. Empty means the
    /// operation is public (it inherits the document's top-level security,
    /// which is currently none).
    pub security: Vec<SecurityRequirement>,
}

impl Default for RouteMeta {
    fn default() -> Self {
        Self {
            summary: None,
            description: None,
            tags: Vec::new(),
            status_code: StatusCode::OK,
            response_model: None,
            request_schema: None,
            request_kind: RequestBodyKind::Json,
            response_schema: None,
            streaming: false,
            websocket: false,
            ws_incoming: None,
            ws_outgoing: None,
            security: Vec::new(),
        }
    }
}

/// A single route: an HTTP method, a path pattern, a handler, and metadata.
///
/// The `path` is local to the router that owns the route until the router tree
/// is flattened, at which point enclosing prefixes are prepended.
#[derive(Clone)]
pub struct Route {
    method: Method,
    path: String,
    handler: HandlerFn,
    meta: RouteMeta,
    request_hooks: Vec<SharedRequestHook>,
    response_hooks: Vec<SharedResponseHook>,
    error_hooks: Vec<SharedErrorHook>,
    validation_hooks: Vec<SharedValidationErrorHook>,
}

impl Route {
    /// Creates a route for `method` at the local `path`, served by `handler`.
    pub fn new(method: Method, path: impl Into<String>, handler: HandlerFn) -> Self {
        Self {
            method,
            path: path.into(),
            handler,
            meta: RouteMeta::default(),
            request_hooks: Vec::new(),
            response_hooks: Vec::new(),
            error_hooks: Vec::new(),
            validation_hooks: Vec::new(),
        }
    }

    scoped_hook_builders!();

    /// Sets the operation summary.
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.meta.summary = Some(summary.into());
        self
    }

    /// Sets the operation description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.meta.description = Some(description.into());
        self
    }

    /// Adds a single tag to the route.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        let tag = tag.into();
        if !self.meta.tags.contains(&tag) {
            self.meta.tags.push(tag);
        }
        self
    }

    /// Sets the default success status code.
    pub fn status_code(mut self, status_code: StatusCode) -> Self {
        self.meta.status_code = status_code;
        self
    }

    /// Declares that this route requires the named security `scheme` with the
    /// given `scopes` (empty for bearer or API-key schemes). Repeatable; a
    /// scheme already present is left unchanged.
    pub fn security(mut self, scheme: impl Into<String>, scopes: &[&str]) -> Self {
        let scheme = scheme.into();
        if !self.meta.security.iter().any(|req| req.scheme == scheme) {
            self.meta.security.push(SecurityRequirement {
                scheme,
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            });
        }
        self
    }

    /// Records the response model type name for documentation.
    pub fn response_model<T: ?Sized>(mut self) -> Self {
        self.meta.response_model = Some(std::any::type_name::<T>());
        self
    }

    /// Records the request body schema generator.
    pub fn request_schema<T: schemars::JsonSchema>(mut self) -> Self {
        self.meta.request_schema = Some(|generator| generator.subschema_for::<T>());
        self
    }

    /// Records the request body schema generator from a thunk (for form bodies).
    pub fn request_schema_fn(mut self, thunk: SchemaThunk) -> Self {
        self.meta.request_schema = Some(thunk);
        self
    }

    /// Sets the request body encoding (`application/json` by default).
    pub fn request_kind(mut self, kind: RequestBodyKind) -> Self {
        self.meta.request_kind = kind;
        self
    }

    /// Records the success response body schema generator.
    pub fn response_schema<T: schemars::JsonSchema>(mut self) -> Self {
        self.meta.response_schema = Some(|generator| generator.subschema_for::<T>());
        self
    }

    /// Marks the response as a stream (documented as `text/event-stream`).
    pub fn streaming(mut self) -> Self {
        self.meta.streaming = true;
        self
    }

    /// Marks the route as a WebSocket endpoint (documented as an AsyncAPI channel).
    pub fn websocket(mut self) -> Self {
        self.meta.websocket = true;
        self
    }

    /// Records the schema of messages the WebSocket receives.
    pub fn ws_incoming<T: schemars::JsonSchema>(mut self) -> Self {
        self.meta.ws_incoming = Some(|generator| generator.subschema_for::<T>());
        self
    }

    /// Records the schema of messages the WebSocket sends.
    pub fn ws_outgoing<T: schemars::JsonSchema>(mut self) -> Self {
        self.meta.ws_outgoing = Some(|generator| generator.subschema_for::<T>());
        self
    }

    /// Returns the HTTP method.
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// Returns the (possibly prefixed) path pattern.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the route's introspection metadata.
    pub fn meta(&self) -> &RouteMeta {
        &self.meta
    }

    /// Returns the type-erased handler.
    pub fn handler(&self) -> &HandlerFn {
        &self.handler
    }

    /// Returns the scoped `on_request` hooks, in firing order (outer to inner).
    pub(crate) fn request_hooks(&self) -> &[SharedRequestHook] {
        &self.request_hooks
    }

    /// Returns the scoped `on_response` hooks, in registration order.
    pub(crate) fn response_hooks(&self) -> &[SharedResponseHook] {
        &self.response_hooks
    }

    /// Returns the scoped `on_error` hooks, in firing order.
    pub(crate) fn error_hooks(&self) -> &[SharedErrorHook] {
        &self.error_hooks
    }

    /// Returns the scoped `on_validation_error` hooks, in firing order.
    pub(crate) fn validation_hooks(&self) -> &[SharedValidationErrorHook] {
        &self.validation_hooks
    }

    /// Reports whether any scoped hook is attached to this route.
    pub(crate) fn has_hooks(&self) -> bool {
        !self.request_hooks.is_empty()
            || !self.response_hooks.is_empty()
            || !self.error_hooks.is_empty()
            || !self.validation_hooks.is_empty()
    }

    /// Prepends `prefix` to the route's path, normalizing the result.
    fn prepend_prefix(mut self, prefix: &str) -> Self {
        self.path = join_paths(prefix, &self.path);
        self
    }

    /// Adds enclosing tags, preserving order and avoiding duplicates.
    fn inherit_tags(mut self, tags: &[String]) -> Self {
        for tag in tags {
            if !self.meta.tags.contains(tag) {
                self.meta.tags.push(tag.clone());
            }
        }
        self
    }

    /// Prepends an enclosing router's scoped hooks ahead of this route's own.
    ///
    /// Inner routers flatten first, so prepending the current (more enclosing)
    /// router's hooks keeps each list ordered outermost to innermost, with the
    /// route's own hooks last.
    fn prepend_hooks(
        mut self,
        request: &[SharedRequestHook],
        response: &[SharedResponseHook],
        error: &[SharedErrorHook],
        validation: &[SharedValidationErrorHook],
    ) -> Self {
        self.request_hooks.splice(0..0, request.iter().cloned());
        self.response_hooks.splice(0..0, response.iter().cloned());
        self.error_hooks.splice(0..0, error.iter().cloned());
        self.validation_hooks
            .splice(0..0, validation.iter().cloned());
        self
    }
}

/// A group of routes sharing a path prefix and a set of tags.
#[derive(Default)]
pub struct Router {
    prefix: String,
    tags: Vec<String>,
    routes: Vec<Route>,
    request_hooks: Vec<SharedRequestHook>,
    response_hooks: Vec<SharedResponseHook>,
    error_hooks: Vec<SharedErrorHook>,
    validation_hooks: Vec<SharedValidationErrorHook>,
}

impl Router {
    /// Creates an empty router with no prefix.
    pub fn new() -> Self {
        Self::default()
    }

    scoped_hook_builders!();

    /// Sets the path prefix applied to every route in this router.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets the tags applied to every route in this router.
    pub fn tags(mut self, tags: &[&str]) -> Self {
        self.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
        self
    }

    /// Adds a route whose path is local to this router.
    pub fn route(mut self, route: Route) -> Self {
        self.routes.push(route);
        self
    }

    /// Nests `child` under this router, composing prefixes and tags.
    pub fn include(mut self, child: Router) -> Self {
        // `child.into_routes` resolves the child prefix and tags onto each child
        // route; this router's own prefix and tags are applied later by
        // `into_routes`.
        self.routes.extend(child.into_routes());
        self
    }

    /// Flattens the router into fully-resolved routes for this level.
    ///
    /// Each route gains this router's prefix and tags. Calling this on the root
    /// router yields the final, fully-qualified route table.
    pub fn into_routes(self) -> Vec<Route> {
        let Router {
            prefix,
            tags,
            routes,
            request_hooks,
            response_hooks,
            error_hooks,
            validation_hooks,
        } = self;
        routes
            .into_iter()
            .map(|route| {
                route
                    .prepend_prefix(&prefix)
                    .inherit_tags(&tags)
                    .prepend_hooks(
                        &request_hooks,
                        &response_hooks,
                        &error_hooks,
                        &validation_hooks,
                    )
            })
            .collect()
    }

    /// Returns the local (unresolved) routes held by this router.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
}

/// Joins a prefix and a path into a single normalized path.
///
/// The boundary between the two is collapsed to a single slash, and a trailing
/// slash is removed except for the root path `/`.
fn join_paths(prefix: &str, path: &str) -> String {
    let head = prefix.trim_end_matches('/');
    let tail = path.trim_start_matches('/');

    let mut combined = String::with_capacity(head.len() + tail.len() + 1);
    combined.push_str(head);
    if !tail.is_empty() {
        combined.push('/');
        combined.push_str(tail);
    }
    if !combined.starts_with('/') {
        combined.insert(0, '/');
    }

    let normalized = combined.trim_end_matches('/');
    if normalized.is_empty() {
        "/".to_owned()
    } else {
        normalized.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::empty;

    fn dummy_handler() -> HandlerFn {
        Arc::new(
            |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async { Ok(empty(StatusCode::OK)) })
            },
        )
    }

    fn get(path: &str) -> Route {
        Route::new(Method::GET, path, dummy_handler())
    }

    #[test]
    fn prefix_is_prepended_to_routes() {
        let routes = Router::new()
            .prefix("/users")
            .tags(&["users"])
            .route(get("/{user_id}"))
            .into_routes();

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path(), "/users/{user_id}");
        assert_eq!(routes[0].meta().tags, vec!["users".to_owned()]);
    }

    #[test]
    fn root_route_drops_trailing_slash() {
        let routes = Router::new().prefix("/users").route(get("/")).into_routes();

        assert_eq!(routes[0].path(), "/users");
    }

    #[test]
    fn nested_include_composes_prefixes_and_tags() {
        let orders = Router::new()
            .prefix("/{user_id}/orders")
            .tags(&["orders"])
            .route(get("/"));

        let routes = Router::new()
            .prefix("/users")
            .tags(&["users"])
            .include(orders)
            .into_routes();

        assert_eq!(routes[0].path(), "/users/{user_id}/orders");
        assert_eq!(
            routes[0].meta().tags,
            vec!["orders".to_owned(), "users".to_owned()]
        );
    }

    #[test]
    fn route_tag_deduplicates_repeated_tags() {
        let route = get("/x").tag("a").tag("a").tag("b");
        assert_eq!(route.meta().tags, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn route_meta_default_has_empty_collections() {
        let meta = RouteMeta::default();
        assert!(meta.summary.is_none());
        assert!(meta.description.is_none());
        assert!(meta.tags.is_empty());
        assert!(meta.request_schema.is_none());
        assert!(meta.response_schema.is_none());
    }

    #[tokio::test]
    async fn router_hooks_propagate_to_routes_outer_to_inner() {
        use crate::hooks::{RequestEvent, RequestInfo};
        use std::sync::Mutex;

        let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let outer_log = log.clone();
        let inner_log = log.clone();

        let inner = Router::new().route(get("/x")).on_request(move |_event| {
            let log = inner_log.clone();
            async move { log.lock().unwrap().push("inner") }
        });
        let outer = Router::new()
            .on_request(move |_event| {
                let log = outer_log.clone();
                async move { log.lock().unwrap().push("outer") }
            })
            .include(inner);

        let routes = outer.into_routes();
        assert_eq!(routes.len(), 1);
        let hooks = routes[0].request_hooks();
        assert_eq!(hooks.len(), 2, "both router hooks attach to the route");

        let info = RequestInfo::new(Method::GET, "/x".into(), Some("/x".into()), None);
        for hook in hooks {
            hook(RequestEvent::new(info.clone())).await;
        }
        assert_eq!(*log.lock().unwrap(), ["outer", "inner"]);
    }
}
