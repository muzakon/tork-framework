//! Request dispatch: turning an HTTP request into a response.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use futures_util::FutureExt;
use http::Request;

use crate::app::{AppInner, fire_request_hooks, fire_response_hooks, request_info};
use crate::body::ReqBody;
use crate::error::Error;
use crate::extract::RequestContext;
use crate::hooks::RequestInfo;
use crate::response::{IntoResponse, Response};
use crate::router::matcher::Match;

impl AppInner {
    /// Routes `request` to its handler and produces a response.
    ///
    /// Routing failures (`404`/`405`) and a handler's own errors are rendered here
    /// via [`render_error`](AppInner::render_error), so the error hooks and any
    /// exception handler run before rendering. This function never returns an
    /// error, so an application error never tears down the connection.
    pub async fn dispatch(&self, request: Request<ReqBody>) -> Response {
        let (head, body) = request.into_parts();
        let path = head.uri.path().to_owned();
        let needs_info = self.needs_request_info();

        match self.matcher().find(&head.method, &path) {
            Match::Found { route, params } => {
                let handler = route.handler().clone();
                // Build metadata when an app-global hook or this route's scoped
                // hooks need it.
                let info = (needs_info || route.has_hooks()).then(|| {
                    request_info(&head.method, &head.uri, &head.headers, Some(route.path().to_owned()))
                });

                // Scoped on_request fires after routing, before the handler.
                if let Some(info) = &info {
                    fire_request_hooks(route.request_hooks(), info).await;
                }

                let started = info.as_ref().map(|_| Instant::now());
                let ctx = RequestContext::new(head, params, self.state().clone(), body);
                let future = handler(ctx);

                // With the panic boundary enabled, a handler panic becomes a 500
                // (and fires the panic hooks) instead of tearing down the task.
                let result = if self.catch_panics() {
                    match AssertUnwindSafe(future).catch_unwind().await {
                        Ok(result) => result,
                        Err(payload) => {
                            if let Some(info) = &info {
                                self.fire_panic(info, &panic_message(payload.as_ref())).await;
                            }
                            return Error::internal("handler panicked").into_response();
                        }
                    }
                } else {
                    future.await
                };

                let response = match result {
                    Ok(response) => response,
                    Err(error) => match &info {
                        Some(info) => self.render_error(error, info, Some(route)).await,
                        None => error.into_response(),
                    },
                };

                // Scoped on_response fires before unwinding back through middleware.
                if let Some(info) = &info {
                    let elapsed = started.map(|start| start.elapsed()).unwrap_or_default();
                    fire_response_hooks(route.response_hooks(), info, response.status(), elapsed)
                        .await;
                }

                response
            }
            Match::MethodNotAllowed => {
                let info =
                    needs_info.then(|| request_info(&head.method, &head.uri, &head.headers, None));
                self.finish_error(Error::method_not_allowed("method not allowed"), info)
                    .await
            }
            Match::NotFound => {
                let info =
                    needs_info.then(|| request_info(&head.method, &head.uri, &head.headers, None));
                self.finish_error(Error::not_found("resource not found"), info)
                    .await
            }
        }
    }

    /// Renders a routing error (no matched route), running the app-global hooks
    /// when request metadata is present.
    async fn finish_error(&self, error: Error, info: Option<RequestInfo>) -> Response {
        match info {
            Some(info) => self.render_error(error, &info, None).await,
            None => error.into_response(),
        }
    }
}

/// Renders a caught panic payload as text for the [`on_panic`](crate::App::on_panic)
/// hooks.
///
/// A panic payload is typically a `&str` or `String`; anything else is reported
/// generically.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, AppInner};
    use crate::body::{ReqBody, box_body};
    use crate::extract::RequestContext;
    use crate::response::Response;
    use crate::router::{BoxFuture, HandlerFn, Route, Router};
    use crate::error::Result;
    use crate::{Method, StatusCode, json_response};

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use std::sync::Arc;

    fn echo_id_handler() -> HandlerFn {
        Arc::new(|ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async move {
                let id = ctx.path_param("user_id").unwrap_or_default().to_owned();
                Ok(json_response(StatusCode::OK, &serde_json::json!({ "id": id })))
            })
        })
    }

    fn test_app() -> AppInner {
        let router = Router::new()
            .prefix("/users")
            .route(Route::new(Method::GET, "/{user_id}", echo_id_handler()));
        App::new().include_router(router).build().unwrap()
    }

    fn request(method: Method, uri: &str) -> http::Request<ReqBody> {
        http::Request::builder()
            .method(method)
            .uri(uri)
            .body(box_body(Full::new(Bytes::new())))
            .unwrap()
    }

    async fn body_to_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn dispatches_to_matching_route() {
        let response = test_app().dispatch(request(Method::GET, "/users/42")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_to_string(response).await.contains("\"42\""));
    }

    #[tokio::test]
    async fn unknown_path_yields_not_found() {
        let response = test_app().dispatch(request(Method::GET, "/nope")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn wrong_method_yields_method_not_allowed() {
        let response = test_app().dispatch(request(Method::POST, "/users/42")).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
