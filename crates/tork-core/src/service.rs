//! Request dispatch: turning an HTTP request into a response.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::time::Instant;

use futures_util::FutureExt;
use http::Request;
use tracing::Instrument;
use tracing::Level;

use crate::app::{fire_request_hooks, fire_response_hooks, request_info, AppInner};
use crate::body::ReqBody;
use crate::error::Error;
use crate::extract::RequestContext;
use crate::hooks::RequestInfo;
use crate::logging::Logger;
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
                    request_info(
                        &head.method,
                        &head.uri,
                        &head.headers,
                        Some(route.path().to_owned()),
                    )
                });

                // Scoped on_request fires after routing, before the handler.
                if let Some(info) = &info {
                    fire_request_hooks(route.request_hooks(), info).await;
                }

                let started = info.as_ref().map(|_| Instant::now());
                let tracing_enabled = tracing::enabled!(Level::INFO);
                let request_meta = if tracing_enabled || self.request_logs() {
                    Some(match &info {
                        Some(info) => DispatchMeta {
                            method: info.method().clone(),
                            request_path: info.path().to_owned(),
                            route_path: info.route().unwrap_or(route.path()).to_owned(),
                            request_id: info.request_id().map(str::to_owned),
                        },
                        None => DispatchMeta {
                            method: head.method.clone(),
                            request_path: head.uri.path().to_owned(),
                            route_path: route.path().to_owned(),
                            request_id: head
                                .headers
                                .get("x-request-id")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned),
                        },
                    })
                } else {
                    None
                };
                let request_started = Instant::now();

                let ctx = RequestContext::new(head, params, self.state().clone(), body);
                let future: Pin<
                    Box<dyn std::future::Future<Output = crate::error::Result<Response>> + Send>,
                > = if tracing_enabled {
                    let meta = request_meta.as_ref().expect("request meta for tracing");
                    let span = tracing::info_span!(
                        "request",
                        method = %meta.method,
                        route = %meta.route_path,
                        request_id = meta.request_id.as_deref().unwrap_or("")
                    );
                    Box::pin(handler(ctx).instrument(span))
                } else {
                    Box::pin(handler(ctx))
                };

                // With the panic boundary enabled, a handler panic becomes a 500
                // (and fires the panic hooks) instead of tearing down the task.
                let result = if self.catch_panics() {
                    match AssertUnwindSafe(future).catch_unwind().await {
                        Ok(result) => result,
                        Err(payload) => {
                            if let Some(info) = &info {
                                self.fire_panic(info, &panic_message(payload.as_ref()))
                                    .await;
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

                // The automatic HTTP request-completed log.
                if self.request_logs() {
                    let meta = request_meta.as_ref().expect("request meta for request log");
                    let status = response.status().as_u16();
                    let mut logger = Logger::framework("HTTP");
                    if let Some(request_id) = &meta.request_id {
                        logger = logger.with_field("request_id", request_id.clone());
                    }
                    logger
                        .info(format!("{} {} {status}", meta.method, meta.request_path))
                        .field("method", meta.method.as_str())
                        .field("path", &meta.request_path)
                        .field("route", &meta.route_path)
                        .field("status", status)
                        .field("duration_ms", request_started.elapsed().as_millis() as u64)
                        .emit();
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

    /// Routes a WebSocket upgrade request over an in-process duplex stream.
    ///
    /// Used by the test client: the matched handler reads the duplex instead of a
    /// real upgraded socket. Returns the handler's response, which is a `101` on a
    /// successful handshake or an error response if the handshake or a dependency
    /// is rejected before the upgrade. (The caller, the test client, lands in a
    /// later commit of this phase.)
    #[allow(dead_code)]
    pub(crate) async fn dispatch_upgrade(
        &self,
        request: Request<ReqBody>,
        duplex: tokio::io::DuplexStream,
    ) -> Response {
        let (head, body) = request.into_parts();
        let path = head.uri.path().to_owned();

        match self.matcher().find(&head.method, &path) {
            Match::Found { route, params } => {
                let handler = route.handler().clone();
                let ctx = RequestContext::with_duplex_upgrade(
                    head,
                    params,
                    self.state().clone(),
                    body,
                    duplex,
                );
                match handler(ctx).await {
                    Ok(response) => response,
                    Err(error) => error.into_response(),
                }
            }
            Match::MethodNotAllowed => {
                Error::method_not_allowed("method not allowed").into_response()
            }
            Match::NotFound => Error::not_found("resource not found").into_response(),
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

struct DispatchMeta {
    method: http::Method,
    request_path: String,
    route_path: String,
    request_id: Option<String>,
}

/// Renders a caught panic payload as text for the [`on_panic`](crate::App::on_panic)
/// hooks.
///
/// A panic payload is typically a `&str` or `String`; anything else is reported
/// generically. The message is truncated to 1024 characters to limit the data
/// exposed to hooks in case the panic message contains sensitive information.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    let max_len = 1024;
    if let Some(message) = payload.downcast_ref::<&str>() {
        let s = (*message).to_owned();
        if s.len() > max_len {
            s[..max_len].to_owned()
        } else {
            s
        }
    } else if let Some(message) = payload.downcast_ref::<String>() {
        if message.len() > max_len {
            message[..max_len].to_owned()
        } else {
            message.clone()
        }
    } else {
        "panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, AppInner};
    use crate::body::{box_body, ReqBody};
    use crate::error::Result;
    use crate::extract::RequestContext;
    use crate::response::Response;
    use crate::router::{BoxFuture, HandlerFn, Route, Router};
    use crate::{json_response, Method, StatusCode};

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use std::sync::Arc;

    fn echo_id_handler() -> HandlerFn {
        Arc::new(
            |ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
                Box::pin(async move {
                    let id = ctx.path_param("user_id").unwrap_or_default().to_owned();
                    Ok(json_response(
                        StatusCode::OK,
                        &serde_json::json!({ "id": id }),
                    ))
                })
            },
        )
    }

    fn test_app() -> AppInner {
        let router = Router::new().prefix("/users").route(Route::new(
            Method::GET,
            "/{user_id}",
            echo_id_handler(),
        ));
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
        let response = test_app()
            .dispatch(request(Method::POST, "/users/42"))
            .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn panic_message_handles_str_payload() {
        use super::panic_message;
        let payload: Box<dyn std::any::Any + Send> = Box::new("oops");
        assert_eq!(panic_message(&*payload), "oops");
    }

    #[test]
    fn panic_message_handles_string_payload() {
        use super::panic_message;
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("from string"));
        assert_eq!(panic_message(&*payload), "from string");
    }

    #[test]
    fn panic_message_handles_unknown_payload() {
        use super::panic_message;
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        assert_eq!(panic_message(&*payload), "panic");
    }
}
