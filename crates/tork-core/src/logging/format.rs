//! Event formatters: a NestJS-style developer console and flat JSON.
//!
//! Tork log events carry a fixed field set (`tork.context`, `tork.fields`, an
//! optional `tork.error`) plus the message. These formatters read those, flatten
//! the serialized field map to top-level keys (JSON) or inline pairs (console), and
//! render one line per event.

use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Field name carrying a Tork logger's context.
const FIELD_CONTEXT: &str = "tork.context";
/// Field name carrying a Tork logger's serialized field map.
const FIELD_FIELDS: &str = "tork.fields";
/// Field name carrying a Tork logger's serialized error.
const FIELD_ERROR: &str = "tork.error";
/// The standard `tracing` message field.
const FIELD_MESSAGE: &str = "message";

/// Process start instant, used to compute the per-line delta.
static START: OnceLock<Instant> = OnceLock::new();
/// Milliseconds (since start) of the previous rendered console line.
static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// The event formatter Tork installs, selected at startup.
pub(crate) enum TorkFormat {
    /// Colored, NestJS-style console lines.
    Console(ConsoleFormat),
    /// One flat JSON object per line.
    Json(JsonFormat),
}

impl<S, N> FormatEvent<S, N> for TorkFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        match self {
            TorkFormat::Console(format) => format.format_event(ctx, writer, event),
            TorkFormat::Json(format) => format.format_event(ctx, writer, event),
        }
    }
}

/// The NestJS-style console formatter.
pub(crate) struct ConsoleFormat {
    pub(crate) color: bool,
}

/// The flat JSON formatter.
pub(crate) struct JsonFormat {
    pub(crate) service_name: String,
}

/// Captures the fields of a single event.
#[derive(Default)]
struct EventVisitor {
    message: Option<String>,
    context: Option<String>,
    fields_json: Option<String>,
    error_json: Option<String>,
}

impl Visit for EventVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let rendered = format!("{value:?}");
        match field.name() {
            FIELD_MESSAGE => self.message = Some(rendered),
            FIELD_CONTEXT => self.context = Some(rendered),
            FIELD_FIELDS => self.fields_json = Some(rendered),
            FIELD_ERROR => self.error_json = Some(rendered),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            FIELD_MESSAGE => self.message = Some(value.to_owned()),
            FIELD_CONTEXT => self.context = Some(value.to_owned()),
            FIELD_FIELDS => self.fields_json = Some(value.to_owned()),
            FIELD_ERROR => self.error_json = Some(value.to_owned()),
            _ => {}
        }
    }
}

impl ConsoleFormat {
    fn format_event<S, N>(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        N: for<'a> FormatFields<'a> + 'static,
    {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let meta = event.metadata();
        let level = *meta.level();
        let context = visitor.context.as_deref().unwrap_or_else(|| meta.target());
        let message = visitor.message.as_deref().unwrap_or("");
        let pid = std::process::id();
        let delta = delta_ms();

        write!(writer, "[Tork] {pid}  - {}  ", console_timestamp())?;
        if self.color {
            write!(writer, "{}{:>5}{} ", level_color(level), level, RESET)?;
        } else {
            write!(writer, "{level:>5} ")?;
        }
        write!(writer, "[{context}] {message}")?;

        if let Some(fields) = &visitor.fields_json {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(fields) {
                for (key, value) in &map {
                    write!(writer, " {key}={}", render_scalar(value))?;
                }
            }
        }
        if let Some(error) = &visitor.error_json {
            write!(writer, " error={error}")?;
        }
        write!(writer, " +{delta}ms")?;
        writeln!(writer)
    }
}

impl JsonFormat {
    fn format_event<S, N>(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        N: for<'a> FormatFields<'a> + 'static,
    {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);

        let meta = event.metadata();
        let mut object = Map::new();
        object.insert("timestamp".to_owned(), Value::String(rfc3339_now()));
        object.insert("level".to_owned(), Value::String(meta.level().to_string()));
        object.insert("service".to_owned(), Value::String(self.service_name.clone()));
        let context = visitor.context.unwrap_or_else(|| meta.target().to_owned());
        object.insert("context".to_owned(), Value::String(context));
        object.insert(
            "message".to_owned(),
            Value::String(visitor.message.unwrap_or_default()),
        );

        // Flatten the logger fields to top-level keys, never overwriting the
        // reserved keys above.
        if let Some(fields) = &visitor.fields_json {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(fields) {
                for (key, value) in map {
                    object.entry(key).or_insert(value);
                }
            }
        }
        if let Some(error) = &visitor.error_json {
            if let Ok(value) = serde_json::from_str::<Value>(error) {
                object.insert("error".to_owned(), value);
            }
        }

        let line = serde_json::to_string(&Value::Object(object)).unwrap_or_default();
        writeln!(writer, "{line}")
    }
}

/// ANSI reset sequence.
const RESET: &str = "\u{1b}[0m";

/// Returns the ANSI color sequence for a level.
fn level_color(level: Level) -> &'static str {
    match level {
        Level::TRACE => "\u{1b}[90m", // bright black
        Level::DEBUG => "\u{1b}[36m", // cyan
        Level::INFO => "\u{1b}[32m",  // green
        Level::WARN => "\u{1b}[33m",  // yellow
        Level::ERROR => "\u{1b}[31m", // red
    }
}

/// Renders a JSON scalar for inline console display (strings without quotes).
fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Milliseconds elapsed since the previous console line (0 on the first line).
fn delta_ms() -> u64 {
    let start = START.get_or_init(Instant::now);
    let now = start.elapsed().as_millis() as u64;
    let last = LAST_MS.swap(now, Ordering::Relaxed);
    now.saturating_sub(last)
}

/// A human-readable local-free timestamp for the console (UTC).
fn console_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:02}/{:02}/{:04}, {:02}:{:02}:{:02}",
        u8::from(now.month()),
        now.day(),
        now.year(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

/// An RFC 3339 timestamp for JSON output.
fn rfc3339_now() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

    /// A `MakeWriter` that appends to a shared buffer, for capturing output.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

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

    /// Renders one event through `format` and returns the captured output.
    fn render(format: TorkFormat) -> String {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = BufWriter(buffer.clone());
        let layer = tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(writer);
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                tork.context = "OrderService",
                tork.fields = "{\"user_id\":42}",
                "Creating order"
            );
        });
        let bytes = buffer.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn json_format_is_flat() {
        let output = render(TorkFormat::Json(JsonFormat {
            service_name: "tork-api".to_owned(),
        }));
        assert!(output.contains("\"level\":\"INFO\""), "{output}");
        assert!(output.contains("\"service\":\"tork-api\""), "{output}");
        assert!(output.contains("\"context\":\"OrderService\""), "{output}");
        assert!(output.contains("\"message\":\"Creating order\""), "{output}");
        // The logger field is flattened to a top-level key.
        assert!(output.contains("\"user_id\":42"), "{output}");
    }

    #[test]
    fn console_format_is_human_readable() {
        let output = render(TorkFormat::Console(ConsoleFormat { color: false }));
        assert!(output.contains("[Tork]"), "{output}");
        assert!(output.contains("[OrderService] Creating order"), "{output}");
        assert!(output.contains("user_id=42"), "{output}");
        assert!(output.contains("ms"), "{output}");
    }
}
