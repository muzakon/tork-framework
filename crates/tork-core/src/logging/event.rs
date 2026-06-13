//! The fluent log-event builder.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tracing::Level;

/// Maximum number of `source()` entries recorded for a logged error, bounding the
/// size of a single error record.
const MAX_ERROR_CHAIN: usize = 16;

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
    ///
    /// The source chain is bounded to [`MAX_ERROR_CHAIN`] entries so a pathological
    /// or deeply nested error cannot produce an unbounded log record. Note that an
    /// error's `Display`/`source` output can carry sensitive data (a database
    /// driver may include a connection string); keep secrets out of error messages,
    /// since logged errors are not redacted the way `5xx` client responses are.
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
            if chain.len() >= MAX_ERROR_CHAIN {
                chain.push(Value::String("... (chain truncated)".to_owned()));
                break;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::LogRecorder;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Debug)]
    struct InnerError;

    impl std::fmt::Display for InnerError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "inner")
        }
    }

    impl std::error::Error for InnerError {}

    #[derive(Debug)]
    struct OuterError;

    impl std::fmt::Display for OuterError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "outer")
        }
    }

    impl std::error::Error for OuterError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&InnerError)
        }
    }

    struct BrokenSerialize;

    impl Serialize for BrokenSerialize {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("nope"))
        }
    }

    #[test]
    fn field_skips_unserializable_values_and_emit_records_error_chain() {
        let recorder = LogRecorder::new();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let event = LogEvent {
            level: Level::ERROR,
            context: Arc::<str>::from("Orders"),
            message: "failed".to_owned(),
            fields: Map::new(),
            error: None,
        }
        .field("ok", 1)
        .field("skip", BrokenSerialize)
        .error(&OuterError);

        tracing::subscriber::with_default(subscriber, move || event.emit());

        let records = recorder.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].context, "Orders");
        assert_eq!(records[0].message, "failed");
        assert_eq!(records[0].fields["ok"], Value::from(1));
    }

    /// An error whose source is another `Deep` of one less depth, forming a chain.
    #[derive(Debug)]
    struct Deep(usize, Option<Box<Deep>>);

    impl std::fmt::Display for Deep {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "level {}", self.0)
        }
    }

    impl std::error::Error for Deep {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|inner| inner as &(dyn std::error::Error + 'static))
        }
    }

    fn deep_chain(depth: usize) -> Deep {
        let mut error = Deep(0, None);
        for level in 1..depth {
            error = Deep(level, Some(Box::new(error)));
        }
        error
    }

    #[test]
    fn error_chain_is_truncated_at_the_cap() {
        let event = LogEvent {
            level: Level::ERROR,
            context: Arc::<str>::from("X"),
            message: "boom".to_owned(),
            fields: Map::new(),
            error: None,
        }
        .error(&deep_chain(100));

        let chain = event.error.as_ref().unwrap()["chain"].as_array().unwrap();
        // MAX_ERROR_CHAIN entries plus the truncation marker.
        assert_eq!(chain.len(), MAX_ERROR_CHAIN + 1);
        assert_eq!(
            chain.last().unwrap(),
            &Value::String("... (chain truncated)".to_owned())
        );
    }
}
