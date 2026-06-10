//! The router data model: [`Route`], [`Router`], and route metadata.
//!
//! A [`Router`] groups routes under a common path prefix and tag set. Routers
//! compose by nesting (`include`), and the whole tree is flattened into a list
//! of fully-qualified [`Route`]s when mounted on the application.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use http::{Method, StatusCode};

use crate::extract::RequestContext;
use crate::response::Response;

pub mod matcher;

/// A boxed, sendable future of a request handler's response.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A type-erased request handler.
///
/// Handlers of every signature are erased to this shape at the router boundary,
/// which is what lets routers store heterogeneous handlers in one table. Exactly
/// one allocation per request is paid here; all resolution inside the handler is
/// statically dispatched.
pub type HandlerFn = Arc<dyn Fn(RequestContext) -> BoxFuture<'static, Response> + Send + Sync>;

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
}

impl Default for RouteMeta {
    fn default() -> Self {
        Self {
            summary: None,
            description: None,
            tags: Vec::new(),
            status_code: StatusCode::OK,
            response_model: None,
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
}

impl Route {
    /// Creates a route for `method` at the local `path`, served by `handler`.
    pub fn new(method: Method, path: impl Into<String>, handler: HandlerFn) -> Self {
        Self {
            method,
            path: path.into(),
            handler,
            meta: RouteMeta::default(),
        }
    }

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

    /// Records the response model type name for documentation.
    pub fn response_model<T: ?Sized>(mut self) -> Self {
        self.meta.response_model = Some(std::any::type_name::<T>());
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
}

/// A group of routes sharing a path prefix and a set of tags.
#[derive(Default)]
pub struct Router {
    prefix: String,
    tags: Vec<String>,
    routes: Vec<Route>,
}

impl Router {
    /// Creates an empty router with no prefix.
    pub fn new() -> Self {
        Self::default()
    }

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
        } = self;
        routes
            .into_iter()
            .map(|route| route.prepend_prefix(&prefix).inherit_tags(&tags))
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
        Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Response> {
            Box::pin(async { empty(StatusCode::OK) })
        })
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
        let routes = Router::new()
            .prefix("/users")
            .route(get("/"))
            .into_routes();

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
}
