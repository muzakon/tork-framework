//! The `AsyncApi` builder and AsyncAPI document assembly.
//!
//! Describes the event-driven side of the API: Server-Sent Events streams and
//! WebSocket connections become AsyncAPI channels, with their message payloads
//! recorded under `components.schemas`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{Map, Value, json};

use tork_core::constants::APPLICATION_JSON;
use tork_core::{
    AsyncApiProvider, BoxFuture, HandlerFn, Method, RequestContext, Response, Result, Route,
    StatusCode, bytes_response,
};

/// AsyncAPI specification version emitted by the document.
const ASYNCAPI_VERSION: &str = "3.0.0";
/// Default path at which the document is served.
const DEFAULT_JSON_PATH: &str = "/asyncapi.json";

/// Configures AsyncAPI document generation.
///
/// The document describes each Server-Sent Events stream and WebSocket route as a
/// channel, with message payload schemas under `components.schemas`.
pub struct AsyncApi {
    title: String,
    version: String,
    description: Option<String>,
    json_path: String,
}

impl Default for AsyncApi {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncApi {
    /// Creates a builder with default title, version, and document path.
    pub fn new() -> Self {
        Self {
            title: "API".to_owned(),
            version: "0.1.0".to_owned(),
            description: None,
            json_path: DEFAULT_JSON_PATH.to_owned(),
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

    /// Sets the path at which the document is served.
    pub fn json(mut self, path: impl Into<String>) -> Self {
        self.json_path = path.into();
        self
    }

    /// Builds the AsyncAPI document for the given routes as a JSON value.
    pub fn build_document(&self, routes: &[Route]) -> Value {
        build_document(self, routes)
    }
}

impl AsyncApiProvider for AsyncApi {
    fn documentation_routes(&self, registered: &[Route]) -> Vec<Route> {
        let document = build_document(self, registered);
        let body = serde_json::to_vec(&document).unwrap_or_default();
        vec![spec_route(&self.json_path, Bytes::from(body))]
    }
}

/// Builds a route that serves a pre-serialized document at `path`.
fn spec_route(path: &str, body: Bytes) -> Route {
    let handler: HandlerFn =
        Arc::new(move |_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            let body = body.clone();
            Box::pin(async move { Ok(bytes_response(StatusCode::OK, APPLICATION_JSON, body)) })
        });

    Route::new(Method::GET, path.to_owned(), handler).summary("AsyncAPI specification")
}

/// Assembles the AsyncAPI document from the route table.
fn build_document(api: &AsyncApi, routes: &[Route]) -> Value {
    // OpenAPI 3 settings make `$ref`s point at `#/components/schemas/...`, which is
    // also where AsyncAPI 3 keeps reusable schemas.
    let mut generator = schemars::generate::SchemaSettings::openapi3().into_generator();
    let mut channels: Map<String, Value> = Map::new();
    let mut operations: Map<String, Value> = Map::new();

    for route in routes {
        let meta = route.meta();
        if !(meta.streaming || meta.websocket) {
            continue;
        }

        let path = route.path();
        let name = channel_name(path);
        let channel_ref = json!({ "$ref": format!("#/channels/{name}") });
        let mut messages: Map<String, Value> = Map::new();

        if meta.streaming {
            if let Some(thunk) = meta.response_schema {
                let payload = thunk(&mut generator).as_value().clone();
                messages.insert("data".to_owned(), json!({ "payload": payload }));
            }
            operations.insert(
                format!("{name}_send"),
                json!({ "action": "send", "channel": channel_ref }),
            );
        } else {
            // WebSocket: receive the incoming message, send the outgoing one.
            if let Some(thunk) = meta.ws_incoming {
                let payload = thunk(&mut generator).as_value().clone();
                messages.insert("incoming".to_owned(), json!({ "payload": payload }));
                operations.insert(
                    format!("{name}_receive"),
                    json!({
                        "action": "receive",
                        "channel": channel_ref,
                        "messages": [{ "$ref": format!("#/channels/{name}/messages/incoming") }],
                    }),
                );
            }
            if let Some(thunk) = meta.ws_outgoing {
                let payload = thunk(&mut generator).as_value().clone();
                messages.insert("outgoing".to_owned(), json!({ "payload": payload }));
                operations.insert(
                    format!("{name}_send"),
                    json!({
                        "action": "send",
                        "channel": channel_ref,
                        "messages": [{ "$ref": format!("#/channels/{name}/messages/outgoing") }],
                    }),
                );
            }
        }

        channels.insert(
            name,
            json!({ "address": path, "messages": Value::Object(messages) }),
        );
    }

    let mut info = Map::new();
    info.insert("title".to_owned(), json!(api.title));
    info.insert("version".to_owned(), json!(api.version));
    if let Some(description) = &api.description {
        info.insert("description".to_owned(), json!(description));
    }

    let mut document = json!({
        "asyncapi": ASYNCAPI_VERSION,
        "info": Value::Object(info),
        "channels": Value::Object(channels),
        "operations": Value::Object(operations),
    });

    let definitions = generator.take_definitions(true);
    if !definitions.is_empty() {
        document["components"] = json!({ "schemas": Value::Object(definitions) });
    }

    document
}

/// Derives a stable channel name from a path, e.g. `/chat/{room}` -> `chat_room`.
fn channel_name(path: &str) -> String {
    let mut name = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if !name.is_empty() {
            name.push('_');
        }
        for ch in segment.chars() {
            if ch.is_ascii_alphanumeric() {
                name.push(ch);
            } else if ch != '{' && ch != '}' {
                name.push('_');
            }
        }
    }
    if name.is_empty() {
        "root".to_owned()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::sync::Arc;
    use tork_core::{RequestContext, Response};

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct ChatIn {
        message: String,
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct ChatOut {
        text: String,
    }

    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Tick {
        n: i64,
    }

    fn dummy_handler() -> HandlerFn {
        Arc::new(|_ctx: RequestContext| -> BoxFuture<'static, Result<Response>> {
            Box::pin(async { Ok(bytes_response(StatusCode::OK, APPLICATION_JSON, Bytes::new())) })
        })
    }

    #[test]
    fn documents_sse_and_websocket_channels() {
        let routes = vec![
            Route::new(Method::GET, "/events", dummy_handler())
                .response_schema::<Tick>()
                .streaming(),
            Route::new(Method::GET, "/chat/{room}", dummy_handler())
                .websocket()
                .ws_incoming::<ChatIn>()
                .ws_outgoing::<ChatOut>(),
        ];

        let document = AsyncApi::new().build_document(&routes);

        assert_eq!(document["asyncapi"], "3.0.0");
        // The SSE channel carries a data message.
        assert_eq!(document["channels"]["events"]["address"], "/events");
        assert!(document["channels"]["events"]["messages"]["data"].is_object());
        // The WebSocket channel carries incoming and outgoing messages.
        assert_eq!(document["channels"]["chat_room"]["address"], "/chat/{room}");
        assert!(document["channels"]["chat_room"]["messages"]["incoming"].is_object());
        assert!(document["channels"]["chat_room"]["messages"]["outgoing"].is_object());
        // Operations describe the direction of each channel.
        assert_eq!(document["operations"]["chat_room_receive"]["action"], "receive");
        assert_eq!(document["operations"]["chat_room_send"]["action"], "send");
        // Message payloads are registered as component schemas.
        assert!(document["components"]["schemas"]["ChatIn"].is_object());
        assert!(document["components"]["schemas"]["ChatOut"].is_object());
        assert!(document["components"]["schemas"]["Tick"].is_object());
    }

    #[test]
    fn ignores_non_realtime_routes_and_omits_empty_components() {
        let routes = vec![Route::new(Method::GET, "/ping", dummy_handler())];

        let document = AsyncApi::new().build_document(&routes);

        assert!(document["channels"].as_object().unwrap().is_empty());
        assert!(document["operations"].as_object().unwrap().is_empty());
        assert!(document.get("components").is_none());
    }

    #[test]
    fn documents_one_sided_stream_and_websocket_channels() {
        let routes = vec![
            Route::new(Method::GET, "/ticks", dummy_handler()).streaming(),
            Route::new(Method::GET, "/in/{room}", dummy_handler())
                .websocket()
                .ws_incoming::<serde_json::Value>(),
            Route::new(Method::GET, "/out/{room}", dummy_handler())
                .websocket()
                .ws_outgoing::<serde_json::Value>(),
        ];

        let document = AsyncApi::new().build_document(&routes);

        assert_eq!(document["channels"]["ticks"]["address"], "/ticks");
        assert!(document["channels"]["ticks"]["messages"]["data"].is_null());
        assert!(document["channels"]["in_room"]["messages"]["incoming"].is_object());
        assert!(document["channels"]["in_room"]["messages"].get("outgoing").is_none());
        assert!(document["channels"]["out_room"]["messages"]["outgoing"].is_object());
        assert!(document["channels"]["out_room"]["messages"].get("incoming").is_none());
        assert_eq!(document["operations"]["in_room_receive"]["action"], "receive");
        assert_eq!(document["operations"]["out_room_send"]["action"], "send");
    }

    #[test]
    fn provider_registers_custom_json_route() {
        let provider = AsyncApi::new()
            .title("Realtime")
            .version("2.0.0")
            .json("/events.json");

        let routes = provider.documentation_routes(&[]);

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path(), "/events.json");
        assert_eq!(routes[0].method(), Method::GET);
    }

    #[test]
    fn build_document_keeps_custom_info_fields() {
        let routes = vec![Route::new(Method::GET, "/events", dummy_handler()).streaming()];
        let document = AsyncApi::new()
            .title("Realtime")
            .version("2.0.0")
            .description("Event stream docs")
            .build_document(&routes);

        assert_eq!(document["info"]["title"], "Realtime");
        assert_eq!(document["info"]["version"], "2.0.0");
        assert_eq!(document["info"]["description"], "Event stream docs");
        assert_eq!(document["channels"]["events"]["address"], "/events");
    }

    #[test]
    fn channel_name_covers_root_and_placeholders() {
        assert_eq!(channel_name("/"), "root");
        assert_eq!(channel_name("/chat/{room}/members"), "chat_room_members");
    }
}
