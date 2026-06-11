//! The fluent log-event builder.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tracing::Level;

/// A log record being built. Add fields and an error, then [`emit`](LogEvent::emit).
///
/// Created by the level methods on [`Logger`](crate::Logger) (`info`, `warn`, ...).
/// Nothing is logged until `emit` is called.
#[must_use = "a LogEvent does nothing until emit() is called"]
pub struct LogEvent {
    pub(crate) level: Level,
    pub(crate) context: Arc<str>,
    pub(crate) message: String,
    pub(crate) fields: Map<String, Value>,
    pub(crate) error: Option<Value>,
}

impl LogEvent {
    /// Attaches a structured field. A non-serializable value is skipped.
    pub fn field<T: Serialize>(mut self, key: &'static str, value: T) -> Self {
        if let Ok(value) = serde_json::to_value(value) {
            self.fields.insert(key.to_owned(), value);
        }
        self
    }

    /// Attaches an error: its type name, message, and source chain.
    pub fn error<E: std::error::Error>(mut self, error: &E) -> Self {
        let mut object = Map::new();
        object.insert(
            "type".to_owned(),
            Value::String(std::any::type_name::<E>().to_owned()),
        );
        object.insert("message".to_owned(), Value::String(error.to_string()));

        let mut chain = Vec::new();
        let mut source = error.source();
        while let Some(error) = source {
            chain.push(Value::String(error.to_string()));
            source = error.source();
        }
        if !chain.is_empty() {
            object.insert("chain".to_owned(), Value::Array(chain));
        }

        self.error = Some(Value::Object(object));
        self
    }

    /// Emits the record at its level to the active subscriber.
    pub fn emit(self) {
        let LogEvent {
            level,
            context,
            message,
            fields,
            error,
        } = self;
        let fields =
            serde_json::to_string(&Value::Object(fields)).unwrap_or_else(|_| "{}".to_owned());
        let error = error.map(|value| serde_json::to_string(&value).unwrap_or_default());
        let context = context.as_ref();

        // The level must be a static call site, so dispatch to the level-specific
        // macro; the message and fields are carried as a fixed set of fields that
        // the Tork formatters recognize and flatten.
        macro_rules! emit_level {
            ($level:ident) => {
                match &error {
                    Some(error) => tracing::$level!(
                        tork.context = %context,
                        tork.fields = %fields,
                        tork.error = %error,
                        "{message}"
                    ),
                    None => tracing::$level!(
                        tork.context = %context,
                        tork.fields = %fields,
                        "{message}"
                    ),
                }
            };
        }

        match level {
            Level::TRACE => emit_level!(trace),
            Level::DEBUG => emit_level!(debug),
            Level::INFO => emit_level!(info),
            Level::WARN => emit_level!(warn),
            Level::ERROR => emit_level!(error),
        }
    }
}
