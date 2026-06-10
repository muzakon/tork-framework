//! Request dispatch: turning an HTTP request into a response.

use http::Request;

use crate::app::{AppInner, request_info};
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
                let info = needs_info.then(|| {
                    request_info(&head.method, &head.uri, &head.headers, Some(route.path().to_owned()))
                });
                let ctx = RequestContext::new(head, params, self.state().clone(), body);
                match handler(ctx).await {
                    Ok(response) => response,
                    Err(error) => self.finish_error(error, info).await,
                }
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

    /// Renders an error, running the error hooks when request metadata is present.
    async fn finish_error(&self, error: Error, info: Option<RequestInfo>) -> Response {
        match info {
            Some(info) => self.render_error(error, &info).await,
            None => error.into_response(),
        }
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
