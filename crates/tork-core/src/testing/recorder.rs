//! A log recorder for asserting on logs in tests.

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use tracing::Event;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer};

/// A single captured log record.
#[derive(Clone, Debug)]
pub struct LogRecord {
    /// The level (`INFO`, `ERROR`, ...).
    pub level: String,
    /// The logger context (for example a service name).
    pub context: String,
    /// The log message.
    pub message: String,
    /// The structured fields.
    pub fields: Map<String, Value>,
}

/// Captures log records for assertions in tests.
///
/// Attach it with [`TestClientBuilder::logger`](super::TestClientBuilder::logger);
/// the client routes its request logs to this recorder for the duration of the
/// test. Works with the default current-thread test runtime.
#[derive(Clone, Default)]
pub struct LogRecorder {
    records: Arc<Mutex<Vec<LogRecord>>>,
}

impl LogRecorder {
    /// Creates an empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of the captured records.
    pub fn records(&self) -> Vec<LogRecord> {
        self.records.lock().expect("recorder mutex poisoned").clone()
    }

    /// Returns `true` if any record has the given context.
    pub fn contains_context(&self, context: &str) -> bool {
        self.records().iter().any(|record| record.context == context)
    }

    /// Returns `true` if any record's message contains `text`.
    pub fn contains_message(&self, text: &str) -> bool {
        self.records().iter().any(|record| record.message.contains(text))
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogRecorder {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = RecordVisitor::default();
        event.record(&mut visitor);
        let record = LogRecord {
            level: event.metadata().level().to_string(),
            context: visitor
                .context
                .unwrap_or_else(|| event.metadata().target().to_owned()),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
        };
        self.records.lock().expect("recorder mutex poisoned").push(record);
    }
}

/// Extracts the Tork fields from an event into a [`LogRecord`].
#[derive(Default)]
struct RecordVisitor {
    message: Option<String>,
    context: Option<String>,
    fields: Map<String, Value>,
}

impl RecordVisitor {
    fn set(&mut self, name: &str, value: String) {
        match name {
            "message" => self.message = Some(value),
            "tork.context" => self.context = Some(value),
            "tork.fields" => {
                if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&value) {
                    self.fields = map;
                }
            }
            _ => {}
        }
    }
}

impl Visit for RecordVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.set(field.name(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field.name(), value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn recorder_helpers_find_context_and_message() {
        let recorder = LogRecorder::new();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(tork.context = "Orders", tork.fields = "{\"id\":1}", "created order");
        });

        assert!(recorder.contains_context("Orders"));
        assert!(recorder.contains_message("created order"));
        assert_eq!(recorder.records()[0].fields["id"], Value::from(1));
    }

    #[test]
    fn visitor_ignores_invalid_json_fields_payload() {
        let mut visitor = RecordVisitor::default();
        visitor.set("tork.fields", "not-json".to_owned());
        visitor.set("message", "hello".to_owned());

        assert_eq!(visitor.message.as_deref(), Some("hello"));
        assert!(visitor.fields.is_empty());
    }
}

/// Asserts a recorder captured a log with the given context and message substring.
///
/// ```ignore
/// assert_logs!(recorder, context = "OrderService", message = "Listing orders");
/// ```
#[macro_export]
macro_rules! assert_logs {
    ($recorder:expr, context = $context:expr, message = $message:expr $(,)?) => {{
        let records = $recorder.records();
        assert!(
            records
                .iter()
                .any(|record| record.context == $context && record.message.contains($message)),
            "no log with context {:?} and message containing {:?}; captured: {:?}",
            $context,
            $message,
            records,
        );
    }};
}
