//! The `OpenApi` builder and specification document assembly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{Map, Value, json};

use tork_core::constants::APPLICATION_JSON;
use tork_core::{
    BoxFuture, HandlerFn, Method, OpenApiProvider, RequestBodyKind, RequestContext, Response,
    Result, Route, StatusCode, bytes_response,
};

/// OpenAPI specification version emitted by the document.
const OPENAPI_VERSION: &str = "3.1.0";
/// Default path at which the specification document is served.
const DEFAULT_JSON_PATH: &str = "/openapi.json";

/// A predicate gating access to the documentation routes. Returning `false`
/// hides the spec and docs UI behind a `404`.
pub(crate) type DocGuard = Arc<dyn Fn(&RequestContext) -> bool + Send + Sync>;

/// Configures OpenAPI document generation.
///
/// The document describes paths, methods, summaries, descriptions, tags, path
/// parameters, and — for routes whose handlers use `#[api_model]` bodies and
/// return types — request and response body schemas under `components.schemas`.
pub struct OpenApi {
    title: String,
    version: String,
    description: Option<String>,
    json_path: String,
    docs_path: Option<String>,
    guard: Option<DocGuard>,
}

impl Default for OpenApi {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenApi {
    /// Creates a builder with default title, version, and document path.
    pub fn new() -> Self {
        Self {
            title: "API".to_owned(),
            version: "0.1.0".to_owned(),
            description: None,
            json_path: DEFAULT_JSON_PATH.to_owned(),
            docs_path: None,
            guard: None,
        }
    }

    /// Sets the API title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the API version.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Sets the API description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the path at which the specification document is served.
    pub fn json(mut self, path: impl Into<String>) -> Self {
        self.json_path = path.into();
        self
    }

    /// Enables the documentation UI, served at `path`.
    pub fn docs(mut self, path: impl Into<String>) -> Self {
        self.docs_path = Some(path.into());
        self
    }

    /// Restricts access to the spec and docs routes to requests the predicate
    /// accepts; rejected requests get a `404` (hiding that the routes exist).
    ///
    /// Use this to keep the API surface from being publicly discoverable — for
    /// example, gate it on a bearer token, an internal network, or an environment
    /// flag. The predicate runs on every request to the documentation routes.
    ///
    /// ```
    /// # use tork_openapi::OpenApi;
    /// let api = OpenApi::new().docs("/docs").protect(|ctx| {
    ///     ctx.headers()
    ///         .get("authorization")
    ///         .and_then(|v| v.to_str().ok())
    ///         == Some("Bearer secret-docs-token")
    /// });
    /// # let _ = api;
    /// ```
    pub fn protect<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&RequestContext) -> bool + Send + Sync + 'static,
    {
        self.guard = Some(Arc::new(predicate));
        self
    }

    /// Builds the OpenAPI document for the given routes as a JSON value.
    pub fn build_document(&self, routes: &[Route]) -> Value {
        build_document(self, routes)
    }
}

impl OpenApiProvider for OpenApi {
    fn documentation_routes(&self, registered: &[Route]) -> Vec<Route> {
        let document = build_document(self, registered);
        let body = serde_json::to_vec(&document).unwrap_or_default();

        let mut routes = vec![spec_route(
            &self.json_path,
            Bytes::from(body),
            self.guard.clone(),
        )];
        if let Some(docs_path) = &self.docs_path {
            routes.push(crate::docs::docs_route(
                docs_path,
                &self.title,
                &self.json_path,
                self.guard.clone(),
            ));
        }
        routes
    }
}

/// Rejects a request with `404` when a documentation guard denies it, hiding the
/// route's existence; returns `Ok(())` when there is no guard or it allows access.
pub(crate) fn check_guard(guard: &Option<DocGuard>, ctx: &RequestContext) -> Result<()> {
    match guard {
        Some(guard) if !guard(ctx) => Err(tork_core::Error::not_found("not found")),
        _ => Ok(()),
    }
}

/// Builds a route that serves a pre-serialized document at `path`.
fn spec_route(path: &str, body: Bytes, guard: Option<DocGuard>) -> Route {
    let handler: HandlerFn =
        Arc::new(move |ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            let body = body.clone();
            let guard = guard.clone();
            Box::pin(async move {
                check_guard(&guard, &ctx)?;
                Ok(bytes_response(StatusCode::OK, APPLICATION_JSON, body))
            })
        });

    Route::new(Method::GET, path.to_owned(), handler).summary("OpenAPI specification")
}

/// Assembles the OpenAPI document from the route table.
fn build_document(api: &OpenApi, routes: &[Route]) -> Value {
    // A single generator collects every model schema; with the OpenAPI 3 settings
    // its `$ref`s already point at `#/components/schemas/...`.
    let mut generator = schemars::generate::SchemaSettings::openapi3().into_generator();
    let mut paths: Map<String, Value> = Map::new();

    for route in routes {
        let path = route.path().to_owned();
        let method = route.method().as_str().to_lowercase();
        let meta = route.meta();

        let mut operation = Map::new();
        if let Some(summary) = &meta.summary {
            operation.insert("summary".to_owned(), json!(summary));
        }
        if let Some(description) = &meta.description {
            operation.insert("description".to_owned(), json!(description));
        }
        if !meta.tags.is_empty() {
            operation.insert("tags".to_owned(), json!(meta.tags));
        }
        operation.insert("operationId".to_owned(), json!(operation_id(&method, &path)));

        let parameters: Vec<Value> = placeholder_names(&path)
            .into_iter()
            .map(|name| {
                json!({
                    "name": name,
                    "in": "path",
                    "required": true,
                    "schema": { "type": "string" },
                })
            })
            .collect();
        if !parameters.is_empty() {
            operation.insert("parameters".to_owned(), json!(parameters));
        }

        if let Some(request_schema) = meta.request_schema {
            let schema = request_schema(&mut generator).as_value().clone();
            // The media type follows the declared body encoding: JSON bodies,
            // urlencoded forms, or multipart forms (whose file fields are marked
            // `format: binary` in the schema).
            let media_type = match meta.request_kind {
                RequestBodyKind::Json => "application/json",
                RequestBodyKind::Form => "application/x-www-form-urlencoded",
                RequestBodyKind::Multipart => "multipart/form-data",
            };
            operation.insert(
                "requestBody".to_owned(),
                json!({
                    "required": true,
                    "content": { media_type: { "schema": schema } },
                }),
            );
        }

        let status = meta.status_code.as_u16().to_string();
        let mut response = Map::new();
        let schema = meta
            .response_schema
            .map(|thunk| thunk(&mut generator).as_value().clone());
        if meta.streaming {
            // A Server-Sent Events stream: each message carries a JSON-encoded
            // value of this schema in its `data:` field.
            response.insert("description".to_owned(), json!("Server-Sent Events stream"));
            if let Some(schema) = schema {
                response.insert(
                    "content".to_owned(),
                    json!({ "text/event-stream": { "schema": schema } }),
                );
            }
        } else {
            let reason = meta.status_code.canonical_reason().unwrap_or("Response");
            response.insert("description".to_owned(), json!(reason));
            if let Some(schema) = schema {
                response.insert(
                    "content".to_owned(),
                    json!({ "application/json": { "schema": schema } }),
                );
            }
        }
        operation.insert(
            "responses".to_owned(),
            json!({ status: Value::Object(response) }),
        );

        let entry = paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(object) = entry.as_object_mut() {
            object.insert(method, Value::Object(operation));
        }
    }

    let mut info = Map::new();
    info.insert("title".to_owned(), json!(api.title));
    info.insert("version".to_owned(), json!(api.version));
    if let Some(description) = &api.description {
        info.insert("description".to_owned(), json!(description));
    }

    let mut document = json!({
        "openapi": OPENAPI_VERSION,
        "info": Value::Object(info),
        "paths": Value::Object(paths),
    });

    // Emit every collected model schema under components.schemas.
    let definitions = generator.take_definitions(true);
    if !definitions.is_empty() {
        document["components"] = json!({ "schemas": Value::Object(definitions) });
    }

    document
}

/// Derives a stable `operationId` from the method and path.
fn operation_id(method: &str, path: &str) -> String {
    let mut id = String::from(method);
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        id.push('_');
        for ch in segment.chars() {
            id.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
        }
    }
    id
}

/// Extracts the placeholder names from a path, e.g. `["user_id"]`.
fn placeholder_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = path.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'{' {
            if let Some(offset) = path[index + 1..].find('}') {
                let inner = &path[index + 1..index + 1 + offset];
                names.push(inner.trim_start_matches('*').to_owned());
                index += offset + 2;
                continue;
            }
        }
        index += 1;
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_handler() -> HandlerFn {
        Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async { Ok(bytes_response(StatusCode::OK, APPLICATION_JSON, Bytes::new())) })
        })
    }

    #[test]
    fn document_describes_routes() {
        let routes = vec![
            Route::new(Method::GET, "/users/{user_id}", dummy_handler())
                .summary("Get user")
                .tag("users"),
        ];

        let document = OpenApi::new()
            .title("My API")
            .version("1.0.0")
            .build_document(&routes);

        assert_eq!(document["openapi"], OPENAPI_VERSION);
        assert_eq!(document["info"]["title"], "My API");
        assert_eq!(document["info"]["version"], "1.0.0");

        let operation = &document["paths"]["/users/{user_id}"]["get"];
        assert_eq!(operation["summary"], "Get user");
        assert_eq!(operation["tags"][0], "users");
        assert_eq!(operation["parameters"][0]["name"], "user_id");
        assert_eq!(operation["parameters"][0]["in"], "path");
        assert!(operation["responses"]["200"].is_object());
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Sample {
        id: i64,
        label: String,
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Inner {
        value: String,
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Outer {
        inner: Inner,
    }

    #[test]
    fn nested_models_are_registered_as_components() {
        let routes =
            vec![Route::new(Method::GET, "/outer", dummy_handler()).response_schema::<Outer>()];

        let schemas = &OpenApi::new().build_document(&routes)["components"]["schemas"];
        assert!(schemas["Outer"].is_object(), "outer missing: {schemas}");
        assert!(schemas["Inner"].is_object(), "nested inner missing: {schemas}");
    }

    #[test]
    fn document_includes_component_schemas() {
        let routes = vec![
            Route::new(Method::POST, "/samples", dummy_handler())
                .request_schema::<Sample>()
                .response_schema::<Sample>(),
        ];

        let document = OpenApi::new().build_document(&routes);

        // The model is registered once under components.schemas.
        assert!(
            document["components"]["schemas"]["Sample"].is_object(),
            "document: {document}"
        );

        let operation = &document["paths"]["/samples"]["post"];
        let request_ref = &operation["requestBody"]["content"]["application/json"]["schema"]["$ref"];
        let response_ref =
            &operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(request_ref, "#/components/schemas/Sample");
        assert_eq!(response_ref, "#/components/schemas/Sample");
    }

    #[test]
    fn multipart_route_documents_form_data_with_binary_file() {
        // A form schema thunk shaped like the one generated by #[derive(FormModel)]:
        // a text field plus a binary file field.
        fn form_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
            schemars::Schema::try_from(json!({
                "type": "object",
                "properties": {
                    "token": { "type": "string" },
                    "file": { "type": "string", "format": "binary" },
                },
                "required": ["token", "file"],
            }))
            .unwrap()
        }

        let routes = vec![
            Route::new(Method::POST, "/files", dummy_handler())
                .request_schema_fn(form_schema)
                .request_kind(RequestBodyKind::Multipart),
        ];

        let document = OpenApi::new().build_document(&routes);
        let content = &document["paths"]["/files"]["post"]["requestBody"]["content"];

        let schema = &content["multipart/form-data"]["schema"];
        assert_eq!(schema["properties"]["file"]["format"], "binary");
        assert!(
            content["application/json"].is_null(),
            "multipart body must not be JSON: {content}"
        );
    }

    #[test]
    fn urlencoded_route_documents_form_content_type() {
        let routes = vec![
            Route::new(Method::POST, "/login", dummy_handler())
                .request_schema::<Sample>()
                .request_kind(RequestBodyKind::Form),
        ];

        let document = OpenApi::new().build_document(&routes);
        let content = &document["paths"]["/login"]["post"]["requestBody"]["content"];

        assert!(
            content["application/x-www-form-urlencoded"]["schema"].is_object(),
            "expected urlencoded body: {content}"
        );
        assert!(content["application/json"].is_null());
    }

    #[test]
    fn streaming_route_documents_event_stream() {
        let routes = vec![
            Route::new(Method::GET, "/stream", dummy_handler())
                .response_schema::<Sample>()
                .streaming(),
        ];

        let document = OpenApi::new().build_document(&routes);
        let response = &document["paths"]["/stream"]["get"]["responses"]["200"];

        assert_eq!(response["description"], "Server-Sent Events stream");
        assert_eq!(
            response["content"]["text/event-stream"]["schema"]["$ref"],
            "#/components/schemas/Sample"
        );
        assert!(
            response["content"]["application/json"].is_null(),
            "streaming response must not be JSON: {response}"
        );
    }

    #[test]
    fn provider_registers_spec_and_docs_routes() {
        let provider = OpenApi::new()
            .title("Docs")
            .version("1.2.3")
            .json("/schema.json")
            .docs("/docs");

        let routes = provider.documentation_routes(&[]);

        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].path(), "/schema.json");
        assert_eq!(routes[1].path(), "/docs");
    }

    #[test]
    fn operation_id_and_placeholder_helpers_cover_edge_cases() {
        assert_eq!(operation_id("patch", "/"), "patch");
        assert_eq!(
            operation_id("get", "/teams/{team-id}/members/{*rest}"),
            "get_teams__team_id__members___rest_"
        );
        assert_eq!(
            placeholder_names("/teams/{team_id}/members/{*rest}"),
            vec!["team_id".to_owned(), "rest".to_owned()]
        );
    }
}
