//! The request context and the [`FromRequest`] dependency-injection trait.
//!
//! Every handler parameter that is not a path parameter is resolved through
//! [`FromRequest`]. Built-in extractors (such as [`State`](crate::State),
//! [`BearerToken`], and [`Json`](crate::Json)) implement it directly, and the
//! `#[tork::dependency]` macro generates an implementation for user-defined
//! dependencies. There is no blanket implementation, which keeps the trait free
//! of coherence conflicts.

use std::net::SocketAddr;
use std::sync::Mutex;

use http::{Extensions, HeaderMap, Method, Uri};
use hyper::upgrade::OnUpgrade;

use crate::body::ReqBody;
use crate::error::{Error, Result};
use crate::state::{AppStateRef, StateMap};
use crate::ws::Upgrade;

pub mod body;
pub mod header;
pub mod path;
pub mod valid;

pub use header::{BearerToken, LastEventId, SseResume};
pub use path::{FromPathParam, __extract_path_param};
pub use valid::Valid;

/// Raw path parameters captured by the router, in match order.
#[derive(Debug, Default, Clone)]
pub struct PathParams {
    entries: Vec<(String, String)>,
}

impl PathParams {
    /// Creates an empty set of path parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a captured parameter and its raw value.
    pub fn push(&mut self, name: String, value: String) {
        self.entries.push((name, value));
    }

    /// Returns the raw value captured for `name`, if any.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Returns `true` if no path parameters were captured.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of captured path parameters.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Everything an extractor needs to resolve a value from the current request.
///
/// Holds the request head (method, URI, headers, extensions), the captured path
/// parameters, a handle to the application state, and the request body. The body
/// can be taken at most once; see [`RequestContext::take_body`].
pub struct RequestContext {
    head: http::request::Parts,
    path_params: PathParams,
    state: AppStateRef,
    body: Mutex<Option<ReqBody>>,
    upgrade: Mutex<Option<Upgrade>>,
}

/// The remote TCP peer address, propagated from the accept loop when present.
#[derive(Clone, Copy)]
pub(crate) struct RequestPeerAddr(pub(crate) SocketAddr);

/// The effective request scheme after trusted proxy normalization.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestScheme {
    Http,
    Https,
}

impl RequestScheme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RequestScheme::Http => "http",
            RequestScheme::Https => "https",
        }
    }
}

pub(crate) fn peer_addr_from_extensions(extensions: &Extensions) -> Option<SocketAddr> {
    extensions.get::<RequestPeerAddr>().map(|peer| peer.0)
}

pub(crate) fn scheme_from_extensions(extensions: &Extensions) -> Option<RequestScheme> {
    extensions.get::<RequestScheme>().copied()
}

impl RequestContext {
    /// Builds a new request context.
    ///
    /// A pending WebSocket upgrade (hyper's `OnUpgrade`, present on an upgrade
    /// request) is taken out of the head's extensions so it can be claimed once
    /// by [`take_upgrade`](RequestContext::take_upgrade).
    pub fn new(
        mut head: http::request::Parts,
        path_params: PathParams,
        state: AppStateRef,
        body: ReqBody,
    ) -> Self {
        let upgrade = head.extensions.remove::<OnUpgrade>().map(Upgrade::Hyper);
        Self {
            head,
            path_params,
            state,
            body: Mutex::new(Some(body)),
            upgrade: Mutex::new(upgrade),
        }
    }

    /// Builds a request context with an in-process WebSocket upgrade.
    ///
    /// Used by the test client to drive a WebSocket handler over an in-memory
    /// duplex stream instead of a real upgraded connection. (The caller lands in a
    /// later commit of this phase.)
    #[allow(dead_code)]
    pub(crate) fn with_duplex_upgrade(
        head: http::request::Parts,
        path_params: PathParams,
        state: AppStateRef,
        body: ReqBody,
        duplex: tokio::io::DuplexStream,
    ) -> Self {
        Self {
            head,
            path_params,
            state,
            body: Mutex::new(Some(body)),
            upgrade: Mutex::new(Some(Upgrade::Duplex(duplex))),
        }
    }

    /// Returns the request method.
    pub fn method(&self) -> &Method {
        &self.head.method
    }

    /// Returns the request URI.
    pub fn uri(&self) -> &Uri {
        &self.head.uri
    }

    /// Returns the request headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.head.headers
    }

    /// Returns the remote TCP peer address, when the request came through the
    /// real server rather than an in-process test transport.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        peer_addr_from_extensions(&self.head.extensions)
    }

    /// Returns the effective request scheme after trusted proxy normalization.
    pub fn scheme(&self) -> Option<&'static str> {
        scheme_from_extensions(&self.head.extensions).map(RequestScheme::as_str)
    }

    /// Returns the full request head.
    pub fn head(&self) -> &http::request::Parts {
        &self.head
    }

    /// Returns the application state map.
    pub fn state(&self) -> &StateMap {
        self.state.as_ref()
    }

    /// Clones a registered resource of type `T` out of the registry.
    ///
    /// # Errors
    ///
    /// Returns an error (code `MISSING_RESOURCE`) if no resource of type `T` was
    /// registered (for example, by a lifespan).
    pub fn resource<T: Clone + Send + Sync + 'static>(&self) -> Result<T> {
        self.state().get::<T>().map(|value| (*value).clone()).ok_or_else(|| {
            Error::internal(format!(
                "resource `{}` was not registered",
                std::any::type_name::<T>()
            ))
            .with_code("MISSING_RESOURCE")
        })
    }

    /// Returns the captured path parameters.
    pub fn path_params(&self) -> &PathParams {
        &self.path_params
    }

    /// Returns the raw value of the path parameter named `name`, if captured.
    pub fn path_param(&self, name: &str) -> Option<&str> {
        self.path_params.get(name)
    }

    /// Takes ownership of the request body.
    ///
    /// # Errors
    ///
    /// Returns a `400 Bad Request` error if the body was already taken, since a
    /// body can only be consumed by a single extractor.
    pub fn take_body(&self) -> Result<ReqBody> {
        self.body
            .lock()
            .expect("request body mutex poisoned")
            .take()
            .ok_or_else(|| Error::bad_request("request body has already been consumed"))
    }

    /// Takes the pending WebSocket upgrade.
    ///
    /// # Errors
    ///
    /// Returns an error (code `NOT_AN_UPGRADE`) if the request is not a WebSocket
    /// upgrade, or if the upgrade was already taken.
    pub(crate) fn take_upgrade(&self) -> Result<Upgrade> {
        self.upgrade
            .lock()
            .expect("request upgrade mutex poisoned")
            .take()
            .ok_or_else(|| {
                Error::bad_request("request is not a WebSocket upgrade").with_code("NOT_AN_UPGRADE")
            })
    }
}

/// Produces a value from the current request to satisfy a handler parameter.
///
/// Implemented directly by built-in extractors and generated by
/// `#[tork::dependency]` for user dependencies. Resolution is always statically
/// dispatched. The returned future is `Send` so the enclosing handler future is
/// `Send`, as required by the server.
pub trait FromRequest: Sized + Send {
    /// Resolves `Self` from the request context.
    ///
    /// An `Err` short-circuits request handling and is rendered as an HTTP error
    /// response.
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send;
}

/// Injects any resource registered as `Arc<T>`.
///
/// Registering a shared value as `Arc<T>` (for example a loaded configuration)
/// lets a handler or service take it by `Arc<T>`, cloning only the pointer per
/// request. This is the idiomatic way to share immutable state cheaply, since the
/// orphan rules prevent a downstream crate from implementing `FromRequest` for
/// `Arc<T>` itself.
impl<T: Send + Sync + 'static> FromRequest for std::sync::Arc<T> {
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let resolved = ctx.resource::<std::sync::Arc<T>>();
        async move { resolved }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::error::ErrorKind;
    use bytes::Bytes;
    use http_body_util::Full;
    use std::sync::Arc;

    fn test_context(path_params: PathParams, body: &'static str) -> RequestContext {
        let head = http::Request::new(()).into_parts().0;
        let body = box_body(Full::new(Bytes::from_static(body.as_bytes())));
        RequestContext::new(head, path_params, Arc::new(StateMap::new()), body)
    }

    #[test]
    fn path_param_lookup_and_parse() {
        let mut params = PathParams::new();
        params.push("user_id".to_owned(), "42".to_owned());
        let ctx = test_context(params, "");

        let parsed: i64 = __extract_path_param(&ctx, "user_id").unwrap();
        assert_eq!(parsed, 42);
    }

    #[test]
    fn invalid_path_param_is_unprocessable() {
        let mut params = PathParams::new();
        params.push("user_id".to_owned(), "not-a-number".to_owned());
        let ctx = test_context(params, "");

        let error = __extract_path_param::<i64>(&ctx, "user_id").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unprocessable);
    }

    #[test]
    fn take_upgrade_errors_without_an_upgrade() {
        let ctx = test_context(PathParams::new(), "");
        let error = ctx.take_upgrade().err().expect("should error without an upgrade");
        assert_eq!(error.code(), "NOT_AN_UPGRADE");
    }

    #[test]
    fn body_can_only_be_taken_once() {
        let ctx = test_context(PathParams::new(), "hello");

        assert!(ctx.take_body().is_ok());
        let error = ctx.take_body().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BadRequest);
    }

    #[test]
    fn resource_is_cloned_from_registry() {
        let mut map = StateMap::new();
        map.insert(42_i64);
        let head = http::Request::new(()).into_parts().0;
        let body = box_body(Full::new(Bytes::from_static(b"")));
        let ctx = RequestContext::new(head, PathParams::new(), Arc::new(map), body);

        assert_eq!(ctx.resource::<i64>().unwrap(), 42);
        assert!(ctx.resource::<String>().is_err());
    }
}
