//! The injectable, context-aware logger.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tracing::Level;

use super::event::LogEvent;
use crate::error::Result;
use crate::extract::{FromRequest, RequestContext};

/// Default context for a logger that was not given one.
const DEFAULT_CONTEXT: &str = "app";
/// Header carrying the request identifier.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// A context-aware logger.
///
/// Injected into handlers and services; the `#[derive(Inject)]` macro gives a
/// `logger: Logger` field the surrounding struct's name as its context. Each log
/// line carries that context and any request-scoped fields (request id, method,
/// path) captured when the logger was resolved.
#[derive(Clone)]
pub struct Logger {
    context: Arc<str>,
    base: Arc<Vec<(&'static str, Value)>>,
}

impl Logger {
    /// Creates a logger with the given context and no base fields.
    pub fn new(context: impl AsRef<str>) -> Self {
        Self {
            context: Arc::from(context.as_ref()),
            base: Arc::new(Vec::new()),
        }
    }

    /// Creates a framework-internal logger (used for startup and request logs).
    pub(crate) fn framework(context: &'static str) -> Self {
        Self::new(context)
    }

    /// Returns the logger's context (the name shown in `[Context]`).
    pub fn context(&self) -> &str {
        &self.context
    }

    /// Returns a logger with a different context, keeping the base fields.
    pub fn for_context(&self, context: impl AsRef<str>) -> Logger {
        Logger {
            context: Arc::from(context.as_ref()),
            base: self.base.clone(),
        }
    }

    /// Returns a logger with an extra field included on every record.
    pub fn with_field<T: Serialize>(&self, key: &'static str, value: T) -> Logger {
        let mut base = (*self.base).clone();
        if let Ok(value) = serde_json::to_value(value) {
            base.push((key, value));
        }
        Logger {
            context: self.context.clone(),
            base: Arc::new(base),
        }
    }

    /// Starts a record at the given level.
    fn event(&self, level: Level, message: impl Into<String>) -> LogEvent {
        let mut fields = Map::new();
        for (key, value) in self.base.iter() {
            fields.insert((*key).to_owned(), value.clone());
        }
        LogEvent {
            level,
            context: self.context.clone(),
            message: message.into(),
            fields,
            error: None,
        }
    }

    /// Starts a `TRACE` record.
    pub fn trace(&self, message: impl Into<String>) -> LogEvent {
        self.event(Level::TRACE, message)
    }

    /// Starts a `DEBUG` record.
    pub fn debug(&self, message: impl Into<String>) -> LogEvent {
        self.event(Level::DEBUG, message)
    }

    /// Starts an `INFO` record.
    pub fn info(&self, message: impl Into<String>) -> LogEvent {
        self.event(Level::INFO, message)
    }

    /// Starts a `WARN` record.
    pub fn warn(&self, message: impl Into<String>) -> LogEvent {
        self.event(Level::WARN, message)
    }

    /// Starts an `ERROR` record.
    pub fn error(&self, message: impl Into<String>) -> LogEvent {
        self.event(Level::ERROR, message)
    }

    /// Builds a span for an operation, to [`enter`](super::LogSpan::enter) a scope.
    pub fn span(&self, name: impl Into<String>) -> super::LogSpan {
        super::LogSpan::new(self.context.clone(), name, &self.base)
    }

    /// Builds a span to [`run`](super::LogSpan::run) a future inside.
    pub fn instrument(&self, name: impl Into<String>) -> super::LogSpan {
        super::LogSpan::new(self.context.clone(), name, &self.base)
    }
}

impl FromRequest for Logger {
    fn from_request(ctx: &RequestContext) -> impl std::future::Future<Output = Result<Self>> + Send {
        let mut base: Vec<(&'static str, Value)> = Vec::new();
        if let Some(request_id) = ctx
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            base.push(("request_id", Value::String(request_id.to_owned())));
        }
        base.push(("method", Value::String(ctx.method().to_string())));
        base.push(("path", Value::String(ctx.uri().path().to_owned())));

        let logger = Logger {
            context: Arc::from(DEFAULT_CONTEXT),
            base: Arc::new(base),
        };
        async move { Ok(logger) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use http_body_util::Full;
    use serde::ser::Error as _;
    use serde::Serializer;
    use super::super::format::{JsonFormat, TorkFormat};
    use crate::{PathParams, RequestContext, StateMap, box_body};
    use crate::extract::FromRequest;
    use std::sync::Arc as StdArc;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    struct BadSerialize;

    impl serde::Serialize for BadSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("nope"))
        }
    }

    impl Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn emits_context_message_and_fields() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TorkFormat::Json(JsonFormat {
                service_name: "svc".to_owned(),
            }))
            .with_writer(BufWriter(buffer.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            Logger::new("PaymentService")
                .with_field("tenant", "acme")
                .info("Charging user")
                .field("user_id", 42)
                .emit();
        });

        let bytes = buffer.lock().unwrap().clone();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains("\"context\":\"PaymentService\""), "{output}");
        assert!(output.contains("\"message\":\"Charging user\""), "{output}");
        assert!(output.contains("\"user_id\":42"), "{output}");
        assert!(output.contains("\"tenant\":\"acme\""), "{output}");
    }

    #[test]
    fn for_context_and_framework_preserve_base_fields() {
        let logger = Logger::framework("startup").with_field("tenant", "acme");
        let relabeled = logger.for_context("payments");

        assert_eq!(logger.context(), "startup");
        assert_eq!(relabeled.context(), "payments");

        let output = {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let layer = tracing_subscriber::fmt::layer()
                .event_format(TorkFormat::Json(JsonFormat {
                    service_name: "svc".to_owned(),
                }))
                .with_writer(BufWriter(buffer.clone()));
            let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            relabeled.info("Boot").emit();
        });
            let bytes = buffer.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        };
        assert!(output.contains("\"context\":\"payments\""), "{output}");
        assert!(output.contains("\"tenant\":\"acme\""), "{output}");
    }

    #[test]
    fn with_field_ignores_unserializable_values() {
        let logger = Logger::new("logger").with_field("tenant", BadSerialize);
        let output = {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let layer = tracing_subscriber::fmt::layer()
                .event_format(TorkFormat::Json(JsonFormat {
                    service_name: "svc".to_owned(),
                }))
                .with_writer(BufWriter(buffer.clone()));
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                logger.info("Hello").emit();
            });
            let bytes = buffer.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        };
        assert!(!output.contains("tenant"), "{output}");
    }

    #[tokio::test]
    async fn from_request_uses_request_metadata_and_default_context() {
        let head = http::Request::builder()
            .method("GET")
            .uri("/logs")
            .header("x-request-id", "req-123")
            .body(())
            .unwrap()
            .into_parts()
            .0;
        let ctx = RequestContext::new(
            head,
            PathParams::new(),
            StdArc::new(StateMap::new()),
            box_body(Full::new(Bytes::new())),
        );

        let logger = Logger::from_request(&ctx).await.unwrap();
        assert_eq!(logger.context(), "app");
        let output = {
            let buffer = Arc::new(Mutex::new(Vec::new()));
            let layer = tracing_subscriber::fmt::layer()
                .event_format(TorkFormat::Json(JsonFormat {
                    service_name: "svc".to_owned(),
                }))
                .with_writer(BufWriter(buffer.clone()));
            let subscriber = tracing_subscriber::registry().with(layer);
            tracing::subscriber::with_default(subscriber, || {
                logger.info("Hello").emit();
            });
            let bytes = buffer.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        };
        assert!(output.contains("\"request_id\":\"req-123\""), "{output}");
        assert!(output.contains("\"method\":\"GET\""), "{output}");
        assert!(output.contains("\"path\":\"/logs\""), "{output}");
    }
}
