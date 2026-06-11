//! The documentation UI route.
//!
//! Serves an HTML page that renders the OpenAPI document with Scalar, loaded
//! from a CDN. The page is generated once and cached.

use std::sync::Arc;

use bytes::Bytes;

use tork_core::constants::TEXT_HTML_UTF8;
use tork_core::{
    BoxFuture, HandlerFn, Method, RequestContext, Response, Result, Route, StatusCode,
    bytes_response,
};

/// CDN URL for the Scalar API reference standalone bundle.
const SCALAR_CDN_URL: &str = "https://cdn.jsdelivr.net/npm/@scalar/api-reference";

/// Builds a route serving the Scalar documentation UI at `path`.
///
/// `spec_url` is the path at which the OpenAPI document is served.
pub(crate) fn docs_route(path: &str, title: &str, spec_url: &str) -> Route {
    let body = Bytes::from(render_html(title, spec_url));

    let handler: HandlerFn =
        Arc::new(move |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            let body = body.clone();
            Box::pin(async move { Ok(bytes_response(StatusCode::OK, TEXT_HTML_UTF8, body)) })
        });

    Route::new(Method::GET, path.to_owned(), handler).summary("API documentation")
}

/// Renders the Scalar documentation page.
fn render_html(title: &str, spec_url: &str) -> String {
    let title = html_escape(title);
    let spec_url = html_escape(spec_url);
    format!(
        "<!doctype html>\n\
         <html>\n\
         <head>\n  \
         <meta charset=\"utf-8\" />\n  \
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  \
         <title>{title}</title>\n\
         </head>\n\
         <body>\n  \
         <script id=\"api-reference\" data-url=\"{spec_url}\"></script>\n  \
         <script src=\"{SCALAR_CDN_URL}\"></script>\n\
         </body>\n\
         </html>\n"
    )
}

/// Minimal HTML escaping for values interpolated into the page.
fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_replaces_reserved_characters() {
        assert_eq!(
            html_escape(r#"<Tork & "Docs">"#),
            "&lt;Tork &amp; &quot;Docs&quot;&gt;"
        );
    }

    #[test]
    fn render_html_embeds_escaped_title_and_spec_url() {
        let html = render_html(r#"Tork "Docs""#, "/openapi.json?x=<tag>");

        assert!(html.contains("<title>Tork &quot;Docs&quot;</title>"));
        assert!(html.contains("data-url=\"/openapi.json?x=&lt;tag&gt;\""));
        assert!(html.contains(SCALAR_CDN_URL));
    }

    #[test]
    fn docs_route_uses_requested_path() {
        let route = docs_route("/docs", "API", "/openapi.json");

        assert_eq!(route.path(), "/docs");
        assert_eq!(route.method(), Method::GET);
        assert_eq!(route.meta().summary.as_deref(), Some("API documentation"));
    }
}
