//! The middleware layer.
//!
//! A middleware is a layer that wraps request handling: it receives the request
//! and a [`Next`] handle, may inspect or modify the request, calls `next` to run
//! the rest of the chain (or short-circuits), and may inspect or modify the
//! response. Middlewares run in registration order, outermost first, and the
//! innermost `next` invokes the route dispatch.

use std::sync::Arc;

use crate::app::AppInner;
use crate::body::ReqBody;
use crate::error::{Error, Result};
use crate::response::Response;
use crate::router::BoxFuture;

pub mod body_limit;
pub mod compression;
pub mod cors;
pub mod https_redirect;
pub mod proxy_headers;
pub mod request_id;
pub mod security_headers;
pub mod timeout;
pub mod trace;
pub mod trusted_host;

pub use body_limit::BodyLimit;
pub use compression::Compression;
pub use cors::Cors;
pub use https_redirect::HttpsRedirect;
pub use proxy_headers::ProxyHeaders;
pub use request_id::RequestId;
pub use security_headers::SecurityHeaders;
pub use timeout::Timeout;
pub use trace::Trace;
pub use trusted_host::TrustedHost;

/// The request type threaded through the middleware chain.
pub type Request = http::Request<ReqBody>;

/// Controls what happens when a middleware whose [`name`](Middleware::name)
/// already exists is registered again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// Keep every registration.
    Allow,
    /// Keep every registration, but log a warning.
    Warn,
    /// Reject the application configuration with an error.
    Reject,
    /// Keep only the most recent registration.
    Replace,
}

/// A request/response middleware layer.
///
/// Built-in middlewares implement this directly; custom middlewares are usually
/// written as an `async fn` annotated with `#[tork::middleware]`, which generates
/// the implementation.
pub trait Middleware: Send + Sync + 'static {
    /// Processes `request`, optionally calling `next` to continue the chain.
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>>;

    /// A stable name used for duplicate detection and diagnostics.
    ///
    /// Built-in middlewares override this with a short name (for example
    /// `"Cors"`); the default is the fully-qualified type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Controls what happens if another middleware with the same name is added.
    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Allow
    }
}

/// A handle to the remainder of the middleware chain.
///
/// Calling [`run`](Next::run) advances to the next middleware, or, once the
/// chain is exhausted, dispatches to the matched route handler.
pub struct Next {
    state: Arc<NextState>,
    index: usize,
}

struct NextState {
    app: Arc<AppInner>,
    stack: Arc<[Arc<dyn Middleware>]>,
}

impl Next {
    /// Creates a chain handle positioned at the first middleware.
    pub(crate) fn new(app: Arc<AppInner>, stack: Arc<[Arc<dyn Middleware>]>) -> Self {
        Self {
            state: Arc::new(NextState { app, stack }),
            index: 0,
        }
    }

    /// Runs the rest of the chain and returns the response.
    ///
    /// If more middlewares remain, the next one is invoked; otherwise the request
    /// is dispatched to its route handler.
    pub fn run(self, request: Request) -> BoxFuture<'static, Result<Response>> {
        match self.state.stack.get(self.index).cloned() {
            Some(middleware) => {
                let next = Next {
                    state: self.state,
                    index: self.index + 1,
                };
                middleware.handle(request, next)
            }
            None => {
                let app = self.state.app.clone();
                Box::pin(async move { Ok(app.dispatch(request).await) })
            }
        }
    }
}

/// Resolves duplicate registrations according to each middleware's policy.
///
/// # Errors
///
/// Returns an error if a middleware whose policy is [`DuplicatePolicy::Reject`]
/// is registered more than once.
pub(crate) fn resolve_duplicates(
    middleware: Vec<Arc<dyn Middleware>>,
) -> Result<Vec<Arc<dyn Middleware>>> {
    let mut resolved: Vec<Arc<dyn Middleware>> = Vec::with_capacity(middleware.len());

    for entry in middleware {
        let name = entry.name();
        let existing = resolved.iter().position(|m| m.name() == name);

        match (existing, entry.duplicate_policy()) {
            (None, _) | (Some(_), DuplicatePolicy::Allow) => resolved.push(entry),
            (Some(_), DuplicatePolicy::Warn) => {
                eprintln!(
                    "tork: middleware `{}` registered more than once",
                    short_name(name)
                );
                resolved.push(entry);
            }
            (Some(index), DuplicatePolicy::Replace) => resolved[index] = entry,
            (Some(_), DuplicatePolicy::Reject) => {
                let short = short_name(name);
                return Err(Error::internal(format!(
                    "Duplicate middleware detected: {short}\n\
                     {short} middleware can only be registered once per scope.\n\
                     Already registered at app level."
                ))
                .with_code("DUPLICATE_MIDDLEWARE"));
            }
        }
    }

    Ok(resolved)
}

/// Returns the last `::`-separated segment of a type name.
fn short_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppInner};
    use crate::body::box_body;
    use crate::constants::TEXT_PLAIN_UTF8;
    use crate::extract::RequestContext;
    use crate::response::bytes_response;
    use crate::router::{HandlerFn, Route, Router};
    use crate::{Method, StatusCode};

    use bytes::Bytes;
    use http::HeaderValue;
    use http_body_util::{BodyExt, Full};

    /// Middleware that records that it ran on the response, then calls `next`.
    struct Mark(&'static str);
    impl Middleware for Mark {
        fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
            let header = self.0;
            Box::pin(async move {
                let mut response = next.run(request).await?;
                response
                    .headers_mut()
                    .append("x-mark", HeaderValue::from_static(header));
                Ok(response)
            })
        }
    }

    /// Middleware that short-circuits without calling `next`.
    struct ShortCircuit;
    impl Middleware for ShortCircuit {
        fn handle(&self, _request: Request, _next: Next) -> BoxFuture<'static, Result<Response>> {
            Box::pin(async { Err(crate::Error::forbidden("blocked")) })
        }
    }

    fn pong_handler() -> HandlerFn {
        std::sync::Arc::new(
            |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async {
                    Ok(bytes_response(
                        StatusCode::OK,
                        TEXT_PLAIN_UTF8,
                        Bytes::from_static(b"pong"),
                    ))
                })
            },
        )
    }

    fn app_with(middlewares: Vec<Box<dyn FnOnce(App) -> App>>) -> std::sync::Arc<AppInner> {
        let mut app = App::new().include_router(Router::new().route(Route::new(
            Method::GET,
            "/",
            pong_handler(),
        )));
        for add in middlewares {
            app = add(app);
        }
        std::sync::Arc::new(app.build().unwrap())
    }

    fn request() -> Request {
        http::Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(box_body(Full::new(Bytes::new())))
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn chain_runs_outermost_first_and_reaches_dispatch() {
        let app = app_with(vec![
            Box::new(|a: App| a.middleware(Mark("outer"))),
            Box::new(|a: App| a.middleware(Mark("inner"))),
        ]);

        let response = app.handle(request()).await;
        assert_eq!(response.status(), StatusCode::OK);

        // Both layers ran (order: inner appends first on the way out, then outer).
        let marks: Vec<_> = response
            .headers()
            .get_all("x-mark")
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(marks, vec!["inner", "outer"]);
        assert_eq!(body_string(response).await, "pong");
    }

    #[tokio::test]
    async fn middleware_can_short_circuit() {
        let app = app_with(vec![Box::new(|a: App| a.middleware(ShortCircuit))]);
        let response = app.handle(request()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// A middleware with a configurable name and duplicate policy.
    struct Policy {
        name: &'static str,
        policy: DuplicatePolicy,
    }
    impl Middleware for Policy {
        fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
            next.run(request)
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn duplicate_policy(&self) -> DuplicatePolicy {
            self.policy
        }
    }

    fn policy(name: &'static str, policy: DuplicatePolicy) -> std::sync::Arc<dyn Middleware> {
        std::sync::Arc::new(Policy { name, policy })
    }

    #[test]
    fn resolve_duplicates_applies_each_policy() {
        // Allow keeps every registration.
        let allowed = resolve_duplicates(vec![
            policy("a", DuplicatePolicy::Allow),
            policy("a", DuplicatePolicy::Allow),
        ])
        .unwrap();
        assert_eq!(allowed.len(), 2);

        // Replace keeps only the most recent.
        let replaced = resolve_duplicates(vec![
            policy("b", DuplicatePolicy::Replace),
            policy("b", DuplicatePolicy::Replace),
        ])
        .unwrap();
        assert_eq!(replaced.len(), 1);

        // Reject fails the configuration.
        assert!(resolve_duplicates(vec![
            policy("c", DuplicatePolicy::Reject),
            policy("c", DuplicatePolicy::Reject)
        ])
        .is_err());

        // Distinct names never collide.
        let distinct = resolve_duplicates(vec![
            policy("x", DuplicatePolicy::Reject),
            policy("y", DuplicatePolicy::Reject),
        ])
        .unwrap();
        assert_eq!(distinct.len(), 2);
    }
}
