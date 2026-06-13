//! Path matching over the flattened route table.
//!
//! One [`matchit::Router`] is built per HTTP method, mapping a path pattern to an
//! index into the flat route table. A method-agnostic index of all paths is kept
//! alongside so that a path which exists under a different method can be reported
//! as `405 Method Not Allowed` rather than `404 Not Found`.

use std::collections::HashMap;

use http::Method;

use crate::error::{Error, Result};
use crate::extract::PathParams;
use crate::router::Route;

/// The outcome of matching a request against the route table.
pub enum Match<'a> {
    /// A route matched; the captured path parameters are included.
    Found {
        /// The matched route.
        route: &'a Route,
        /// Path parameters captured from the URL.
        params: PathParams,
    },
    /// The path exists, but not for the requested method.
    MethodNotAllowed,
    /// No route matched the path.
    NotFound,
}

/// Compiled, per-method path matcher over a flat route table.
pub struct Matcher {
    by_method: HashMap<Method, matchit::Router<usize>>,
    all_paths: matchit::Router<()>,
    routes: Vec<Route>,
}

impl Matcher {
    /// Builds a matcher from fully-qualified routes.
    ///
    /// # Errors
    ///
    /// Returns an internal error if a route path is not a valid pattern or if the
    /// same method and path are registered twice.
    pub fn build(routes: Vec<Route>) -> Result<Self> {
        let mut by_method: HashMap<Method, matchit::Router<usize>> = HashMap::new();
        let mut all_paths: matchit::Router<()> = matchit::Router::new();

        for (index, route) in routes.iter().enumerate() {
            let method_router = by_method.entry(route.method().clone()).or_default();

            method_router.insert(route.path(), index).map_err(|error| {
                Error::internal(format!(
                    "failed to register route {} {}: {error}",
                    route.method(),
                    route.path()
                ))
            })?;

            // The same path may legitimately appear under multiple methods, so a
            // duplicate insert here is expected and ignored.
            let _ = all_paths.insert(route.path(), ());
        }

        Ok(Self {
            by_method,
            all_paths,
            routes,
        })
    }

    /// Matches `method` and `path` against the route table.
    pub fn find(&self, method: &Method, path: &str) -> Match<'_> {
        // Reject null bytes in the path — a reverse proxy should never forward
        // them, but if one does, the router must not interpret the path beyond
        // the null (which could bypass route guards).
        if path.contains('\0') {
            return Match::NotFound;
        }
        if let Some(method_router) = self.by_method.get(method) {
            if let Ok(matched) = method_router.at(path) {
                let mut params = PathParams::new();
                for (name, value) in matched.params.iter() {
                    params.push(name.to_owned(), value.to_owned());
                }
                return Match::Found {
                    route: &self.routes[*matched.value],
                    params,
                };
            }

            if let Some(normalized) = normalized_request_path(path) {
                if let Ok(matched) = method_router.at(normalized) {
                    let mut params = PathParams::new();
                    for (name, value) in matched.params.iter() {
                        params.push(name.to_owned(), value.to_owned());
                    }
                    return Match::Found {
                        route: &self.routes[*matched.value],
                        params,
                    };
                }
            }

            if let Some(collapsed) = collapse_double_slashes(path) {
                if let Ok(matched) = method_router.at(&collapsed) {
                    let mut params = PathParams::new();
                    for (name, value) in matched.params.iter() {
                        params.push(name.to_owned(), value.to_owned());
                    }
                    return Match::Found {
                        route: &self.routes[*matched.value],
                        params,
                    };
                }
            }
        }

        if self.all_paths.at(path).is_ok() {
            Match::MethodNotAllowed
        } else if let Some(normalized) = normalized_request_path(path) {
            if self.all_paths.at(normalized).is_ok() {
                Match::MethodNotAllowed
            } else {
                Match::NotFound
            }
        } else if let Some(collapsed) = collapse_double_slashes(path) {
            if self.all_paths.at(&collapsed).is_ok() {
                Match::MethodNotAllowed
            } else {
                Match::NotFound
            }
        } else {
            Match::NotFound
        }
    }

    /// Returns the flat route table.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
}

/// Returns a trailing-slash-trimmed view of `path` when normalization is needed.
///
/// Registered paths drop their trailing slash (except the root), so incoming
/// paths ending with `/` are retried without allocating in the common case where
/// the request is already normalized.
fn normalized_request_path(path: &str) -> Option<&str> {
    if path == "/" || !path.ends_with('/') {
        return None;
    }

    let trimmed = path.trim_end_matches('/');
    Some(if trimmed.is_empty() { "/" } else { trimmed })
}

/// Collapses consecutive slashes in `path` into a single `/`.
///
/// A path like `//api//users` is normalized to `/api/users`. Returns `None`
/// when the path is already single-slash-normal (the common case).
fn collapse_double_slashes(path: &str) -> Option<String> {
    if !path.contains("//") {
        return None;
    }
    let collapsed: String = path
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    Some(if collapsed.is_empty() {
        "/".to_owned()
    } else {
        collapsed
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::extract::RequestContext;
    use crate::response::{empty, Response};
    use crate::router::{BoxFuture, HandlerFn};
    use http::StatusCode;
    use std::sync::Arc;

    fn dummy_handler() -> HandlerFn {
        Arc::new(
            |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async { Ok(empty(StatusCode::OK)) })
            },
        )
    }

    fn matcher() -> Matcher {
        Matcher::build(vec![Route::new(
            Method::GET,
            "/users/{user_id}",
            dummy_handler(),
        )])
        .unwrap()
    }

    #[test]
    fn matches_and_captures_params() {
        match matcher().find(&Method::GET, "/users/42") {
            Match::Found { params, .. } => assert_eq!(params.get("user_id"), Some("42")),
            _ => panic!("expected a match"),
        }
    }

    #[test]
    fn trailing_slash_is_ignored() {
        assert!(matches!(
            matcher().find(&Method::GET, "/users/42/"),
            Match::Found { .. }
        ));
    }

    #[test]
    fn wrong_method_is_method_not_allowed() {
        assert!(matches!(
            matcher().find(&Method::POST, "/users/42"),
            Match::MethodNotAllowed
        ));
    }

    #[test]
    fn unknown_path_is_not_found() {
        assert!(matches!(
            matcher().find(&Method::GET, "/unknown"),
            Match::NotFound
        ));
    }

    #[test]
    fn build_rejects_duplicate_same_method_and_path() {
        let routes = vec![
            Route::new(Method::GET, "/users/{user_id}", dummy_handler()),
            Route::new(Method::GET, "/users/{user_id}", dummy_handler()),
        ];
        let err = match Matcher::build(routes) {
            Ok(_) => panic!("expected duplicate route registration to fail"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("failed to register route GET /users/{user_id}"));
    }

    #[test]
    fn normalized_request_path_covers_root_and_trailing_slashes() {
        assert_eq!(normalized_request_path("/"), None);
        assert_eq!(normalized_request_path("/users"), None);
        assert_eq!(normalized_request_path("/users/"), Some("/users"));
        assert_eq!(normalized_request_path("/users///"), Some("/users"));
    }

    #[test]
    fn root_path_matches_and_method_not_allowed_uses_all_paths() {
        let routes = vec![
            Route::new(Method::GET, "/", dummy_handler()),
            Route::new(Method::POST, "/users", dummy_handler()),
        ];
        let matcher = Matcher::build(routes).unwrap();
        assert!(matches!(
            matcher.find(&Method::GET, "/"),
            Match::Found { .. }
        ));
        assert!(matches!(
            matcher.find(&Method::POST, "/"),
            Match::MethodNotAllowed
        ));
        assert!(matches!(
            matcher.find(&Method::GET, "/users/"),
            Match::MethodNotAllowed
        ));
    }
}
