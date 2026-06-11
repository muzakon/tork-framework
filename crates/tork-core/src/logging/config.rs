//! Logging configuration.
//!
//! [`LoggerConfig`] describes how the application logs: the level, the output
//! format (a colored developer console or structured JSON), whether HTTP request
//! logs are emitted, and optional file and OpenTelemetry sinks. It is a plain
//! value, so an application can build it directly or from its own settings.

use std::path::PathBuf;

/// Default log level when none is configured.
pub(crate) const DEFAULT_LEVEL: &str = "info";
/// Default service name reported in structured logs.
pub(crate) const DEFAULT_SERVICE_NAME: &str = "app";
/// Default file name prefix for the rolling file sink.
pub(crate) const DEFAULT_FILE_PREFIX: &str = "app";

/// The console output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Choose automatically: a pretty console when attached to a terminal,
    /// structured JSON otherwise.
    #[default]
    Auto,
    /// A colored, human-readable console line.
    Pretty,
    /// A terse single-line console format.
    Compact,
    /// One JSON object per line.
    Json,
}

/// How often a file log is rolled over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    /// Never roll over; a single file grows.
    Never,
    /// Roll over hourly.
    Hourly,
    /// Roll over daily.
    #[default]
    Daily,
}

/// Configuration for a rolling file log sink.
// Fields are read by the file sink, which lands in a later commit of this phase.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FileLogConfig {
    pub(crate) directory: PathBuf,
    pub(crate) prefix: String,
    pub(crate) rotation: Rotation,
    pub(crate) non_blocking: bool,
}

impl FileLogConfig {
    /// Creates a file sink writing into `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            prefix: DEFAULT_FILE_PREFIX.to_owned(),
            rotation: Rotation::default(),
            non_blocking: true,
        }
    }

    /// Sets the file name prefix.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets the rotation policy.
    pub fn rotation(mut self, rotation: Rotation) -> Self {
        self.rotation = rotation;
        self
    }

    /// Sets whether file writes go through a non-blocking background worker.
    pub fn non_blocking(mut self, non_blocking: bool) -> Self {
        self.non_blocking = non_blocking;
        self
    }
}

/// Configuration for OpenTelemetry trace export (effective with the `otel` feature).
// Fields are read by the OTel layer, which lands behind the `otel` feature later.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub(crate) enabled: bool,
    pub(crate) otlp_endpoint: String,
    pub(crate) service_name: String,
}

impl TelemetryConfig {
    /// Creates a telemetry configuration exporting to `otlp_endpoint`.
    pub fn new(otlp_endpoint: impl Into<String>) -> Self {
        Self {
            enabled: true,
            otlp_endpoint: otlp_endpoint.into(),
            service_name: DEFAULT_SERVICE_NAME.to_owned(),
        }
    }

    /// Sets the reported service name.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Enables or disables export.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// How the application logs.
#[derive(Debug, Clone)]
pub struct LoggerConfig {
    pub(crate) level: String,
    pub(crate) format: LogFormat,
    pub(crate) color: bool,
    pub(crate) service_name: String,
    // `request_logs`, `file`, and `telemetry` are consumed by request logging, the
    // file sink, and the OTel layer in later commits of this phase.
    #[allow(dead_code)]
    pub(crate) request_logs: bool,
    #[allow(dead_code)]
    pub(crate) include_source: bool,
    #[allow(dead_code)]
    pub(crate) include_thread_ids: bool,
    pub(crate) non_blocking: bool,
    #[allow(dead_code)]
    pub(crate) file: Option<FileLogConfig>,
    #[allow(dead_code)]
    pub(crate) telemetry: Option<TelemetryConfig>,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL.to_owned(),
            format: LogFormat::Auto,
            color: true,
            service_name: DEFAULT_SERVICE_NAME.to_owned(),
            request_logs: true,
            include_source: false,
            include_thread_ids: false,
            non_blocking: false,
            file: None,
            telemetry: None,
        }
    }
}

impl LoggerConfig {
    /// Creates a configuration with the default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum log level (`trace`/`debug`/`info`/`warn`/`error`, or any
    /// `RUST_LOG`-style directive).
    pub fn level(mut self, level: impl Into<String>) -> Self {
        self.level = level.into();
        self
    }

    /// Sets the output format.
    pub fn format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    /// Enables or disables ANSI color in the console format.
    pub fn color(mut self, color: bool) -> Self {
        self.color = color;
        self
    }

    /// Sets the service name reported in structured logs.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Enables or disables the automatic HTTP request-completed log.
    pub fn request_logs(mut self, enabled: bool) -> Self {
        self.request_logs = enabled;
        self
    }

    /// Includes the source file and line in each record.
    pub fn include_source(mut self, include: bool) -> Self {
        self.include_source = include;
        self
    }

    /// Includes the thread id in each record.
    pub fn include_thread_ids(mut self, include: bool) -> Self {
        self.include_thread_ids = include;
        self
    }

    /// Writes through a non-blocking background worker.
    pub fn non_blocking(mut self, non_blocking: bool) -> Self {
        self.non_blocking = non_blocking;
        self
    }

    /// Adds a rolling file sink.
    pub fn file(mut self, file: FileLogConfig) -> Self {
        self.file = Some(file);
        self
    }

    /// Adds OpenTelemetry trace export (effective with the `otel` feature).
    pub fn telemetry(mut self, telemetry: TelemetryConfig) -> Self {
        self.telemetry = Some(telemetry);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let config = LoggerConfig::new();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, LogFormat::Auto);
        assert!(config.color);
        assert!(config.request_logs);
        assert!(config.file.is_none());
        assert!(config.telemetry.is_none());
    }

    #[test]
    fn builders_set_fields() {
        let config = LoggerConfig::new()
            .level("debug")
            .format(LogFormat::Json)
            .service_name("tork-api")
            .request_logs(false)
            .file(FileLogConfig::new("./logs").prefix("api").rotation(Rotation::Hourly));

        assert_eq!(config.level, "debug");
        assert_eq!(config.format, LogFormat::Json);
        assert_eq!(config.service_name, "tork-api");
        assert!(!config.request_logs);
        let file = config.file.expect("file sink");
        assert_eq!(file.prefix, "api");
        assert_eq!(file.rotation, Rotation::Hourly);
    }

    #[test]
    fn log_format_deserializes_from_lowercase() {
        let format: LogFormat = serde_json::from_str("\"json\"").unwrap();
        assert_eq!(format, LogFormat::Json);
        let format: LogFormat = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(format, LogFormat::Auto);
    }
}
