//! Scoped spans built from a [`Logger`](crate::Logger).
//!
//! A span groups the logs of an operation and carries its fields for trace
//! exporters. Enter one for a synchronous scope, or wrap a future so the span
//! stays active across its `await` points.

use std::future::Future;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tracing::Instrument;
use tracing::span::EnteredSpan;

/// A span being built. Add fields, then [`enter`](LogSpan::enter) a scope or
/// [`run`](LogSpan::run) a future inside it.
#[must_use = "a LogSpan does nothing until enter() or run() is called"]
pub struct LogSpan {
    context: Arc<str>,
    name: String,
    fields: Map<String, Value>,
}

impl LogSpan {
    /// Builds a span for an operation named `name`, seeded with `base` fields.
    pub(crate) fn new(context: Arc<str>, name: impl Into<String>, base: &[(&'static str, Value)]) -> Self {
        let mut fields = Map::new();
        for (key, value) in base {
            fields.insert((*key).to_owned(), value.clone());
        }
        Self {
            context,
            name: name.into(),
            fields,
        }
    }

    /// Attaches a field to the span. A non-serializable value is skipped.
    pub fn field<T: Serialize>(mut self, key: &'static str, value: T) -> Self {
        if let Ok(value) = serde_json::to_value(value) {
            self.fields.insert(key.to_owned(), value);
        }
        self
    }

    /// Builds the underlying `tracing` span.
    fn build(&self) -> tracing::Span {
        let fields =
            serde_json::to_string(&Value::Object(self.fields.clone())).unwrap_or_else(|_| "{}".to_owned());
        let context = self.context.as_ref();
        let name = self.name.as_str();
        tracing::info_span!(
            "op",
            tork.context = %context,
            tork.op = %name,
            tork.fields = %fields
        )
    }

    /// Enters the span, returning a guard that exits it when dropped.
    pub fn enter(self) -> EnteredSpan {
        self.build().entered()
    }

    /// Runs `future` inside the span, so all of its logs are grouped under it.
    pub async fn run<F: Future>(self, future: F) -> F::Output {
        future.instrument(self.build()).await
    }
}

#[cfg(test)]
mod tests {
    use crate::logging::Logger;

    #[tokio::test]
    async fn instrument_runs_the_future_and_returns_its_value() {
        let logger = Logger::new("Worker");
        let output = logger
            .instrument("job")
            .field("attempt", 1)
            .run(async { 21 * 2 })
            .await;
        assert_eq!(output, 42);
    }

    #[test]
    fn span_enters_a_scope() {
        let logger = Logger::new("Worker");
        let guard = logger.span("scope").field("key", "value").enter();
        drop(guard);
    }
}
