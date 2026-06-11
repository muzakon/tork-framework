//! Installs the global `tracing` subscriber from a [`LoggerConfig`].

use std::io::IsTerminal;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use super::config::{FileLogConfig, LogFormat, LoggerConfig, Rotation};
use super::format::{ConsoleFormat, JsonFormat, TorkFormat};

/// A boxed layer over the registry.
type BoxLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Keeps logging resources (such as the non-blocking writer workers) alive for the
/// lifetime of the application. Dropping it flushes and stops those workers.
#[must_use = "dropping the LoggerHandle stops background log workers"]
pub(crate) struct LoggerHandle {
    _guards: Vec<WorkerGuard>,
    #[cfg(feature = "otel")]
    otel_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

#[cfg(feature = "otel")]
impl Drop for LoggerHandle {
    fn drop(&mut self) {
        // Flush and stop the trace exporter on shutdown.
        if let Some(provider) = &self.otel_provider {
            let _ = provider.shutdown();
        }
    }
}

/// Installs the global subscriber from `config`.
///
/// Returns a [`LoggerHandle`] that must be kept alive while the application runs.
/// Installation is best-effort: if a global subscriber is already set (for example
/// in tests, or when the host application configured its own), this is a no-op.
pub(crate) fn install(config: &LoggerConfig) -> LoggerHandle {
    let mut guards = Vec::new();
    let mut layers: Vec<BoxLayer> = Vec::new();
    layers.push(stdout_layer(config, &mut guards));
    if let Some(file) = &config.file {
        layers.push(file_layer(config, file, &mut guards));
    }

    #[cfg(feature = "otel")]
    let otel_provider = config.telemetry.as_ref().and_then(|telemetry| {
        otel_layer(config, telemetry).map(|(layer, provider)| {
            layers.push(layer);
            provider
        })
    });

    // A failure means a subscriber was already installed; that is fine.
    let _ = Registry::default().with(layers).try_init();
    LoggerHandle {
        _guards: guards,
        #[cfg(feature = "otel")]
        otel_provider,
    }
}

/// Builds the OpenTelemetry trace-export layer and its provider.
#[cfg(feature = "otel")]
fn otel_layer(
    config: &LoggerConfig,
    telemetry: &super::config::TelemetryConfig,
) -> Option<(BoxLayer, opentelemetry_sdk::trace::SdkTracerProvider)> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig as _;

    if !telemetry.enabled {
        return None;
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&telemetry.otlp_endpoint)
        .build()
        .ok()?;
    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(telemetry.service_name.clone())
        .build();
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer("tork");
    let layer = tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_filter(env_filter(&config.level))
        .boxed();
    Some((layer, provider))
}

/// Builds the level/`RUST_LOG` filter (a fresh value per layer, as it is not
/// clonable).
fn env_filter(level: &str) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Builds the console/JSON layer that writes to standard output.
fn stdout_layer(config: &LoggerConfig, guards: &mut Vec<WorkerGuard>) -> BoxLayer {
    let format = build_format(config);
    if config.non_blocking {
        let (writer, guard) = tracing_appender::non_blocking(std::io::stdout());
        guards.push(guard);
        tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(writer)
            .with_filter(env_filter(&config.level))
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(std::io::stdout)
            .with_filter(env_filter(&config.level))
            .boxed()
    }
}

/// Builds the rolling JSON file layer.
fn file_layer(config: &LoggerConfig, file: &FileLogConfig, guards: &mut Vec<WorkerGuard>) -> BoxLayer {
    // Ensure the directory exists; ignore the error and let writing surface it.
    let _ = std::fs::create_dir_all(&file.directory);
    let appender = match file.rotation {
        Rotation::Never => rolling::never(&file.directory, &file.prefix),
        Rotation::Hourly => rolling::hourly(&file.directory, &file.prefix),
        Rotation::Daily => rolling::daily(&file.directory, &file.prefix),
    };
    // Files are always structured JSON.
    let format = TorkFormat::Json(JsonFormat {
        service_name: config.service_name.clone(),
    });
    if file.non_blocking {
        let (writer, guard) = tracing_appender::non_blocking(appender);
        guards.push(guard);
        tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(writer)
            .with_filter(env_filter(&config.level))
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(appender)
            .with_filter(env_filter(&config.level))
            .boxed()
    }
}

/// Resolves the configured format into a concrete event formatter, choosing a
/// console or JSON renderer (and honoring `Auto` against the terminal).
fn build_format(config: &LoggerConfig) -> TorkFormat {
    let json = match config.format {
        LogFormat::Json => true,
        LogFormat::Pretty | LogFormat::Compact => false,
        LogFormat::Auto => !std::io::stdout().is_terminal(),
    };

    if json {
        TorkFormat::Json(JsonFormat {
            service_name: config.service_name.clone(),
        })
    } else {
        TorkFormat::Console(ConsoleFormat {
            color: config.color && std::io::stdout().is_terminal(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn env_filter_uses_explicit_level_and_fallback() {
        std::env::remove_var("RUST_LOG");
        let debug = env_filter("debug");
        assert_eq!(debug.to_string(), "debug");

        let fallback = env_filter("definitely not a log filter");
        assert_eq!(fallback.to_string(), "info");

        std::env::set_var("RUST_LOG", "warn,tork=trace");
        let from_env = env_filter("error");
        assert_eq!(from_env.to_string(), "warn,tork=trace");
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn build_format_honors_explicit_json_and_console_preferences() {
        let json = build_format(&LoggerConfig::new().format(LogFormat::Json).service_name("svc"));
        match json {
            TorkFormat::Json(format) => assert_eq!(format.service_name, "svc"),
            other => panic!("expected json formatter, got {other:?}"),
        }

        let console = build_format(&LoggerConfig::new().format(LogFormat::Pretty).color(false));
        match console {
            TorkFormat::Console(format) => assert!(!format.color),
            other => panic!("expected console formatter, got {other:?}"),
        }

        let compact = build_format(&LoggerConfig::new().format(LogFormat::Compact));
        assert!(matches!(compact, TorkFormat::Console(_)));
    }

    #[test]
    fn stdout_and_file_layers_cover_blocking_and_non_blocking_paths() {
        let mut guards = Vec::new();
        let config = LoggerConfig::new().format(LogFormat::Json).non_blocking(true);
        let _ = stdout_layer(&config, &mut guards);
        assert_eq!(guards.len(), 1);

        let mut no_guards = Vec::new();
        let _ = stdout_layer(&LoggerConfig::new().format(LogFormat::Pretty), &mut no_guards);
        assert!(no_guards.is_empty());

        let dir = tempfile::tempdir().unwrap();
        let file = FileLogConfig::new(dir.path())
            .prefix("hourly")
            .rotation(Rotation::Hourly)
            .non_blocking(true);
        let mut file_guards = Vec::new();
        let _ = file_layer(&LoggerConfig::new().level("debug"), &file, &mut file_guards);
        assert_eq!(file_guards.len(), 1);

        let daily = FileLogConfig::new(dir.path())
            .prefix("daily")
            .rotation(Rotation::Daily)
            .non_blocking(false);
        let mut daily_guards = Vec::new();
        let _ = file_layer(&LoggerConfig::new(), &daily, &mut daily_guards);
        assert!(daily_guards.is_empty());
    }

    #[test]
    fn install_accepts_file_sink_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let config = LoggerConfig::new().file(
            FileLogConfig::new(dir.path())
                .prefix("app")
                .rotation(Rotation::Never)
                .non_blocking(false),
        );

        let _handle = install(&config);
    }

    #[test]
    fn file_layer_writes_json_records() {
        let dir = tempfile::tempdir().unwrap();
        let appender = rolling::never(dir.path(), "test.log");
        let layer = tracing_subscriber::fmt::layer()
            .event_format(TorkFormat::Json(JsonFormat {
                service_name: "svc".to_owned(),
            }))
            .with_writer(appender);
        let subscriber = Registry::default().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(tork.context = "FileTest", tork.fields = "{}", "to a file");
        });

        // The blocking appender wrote synchronously; read it back.
        let path = dir.path().join("test.log");
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"context\":\"FileTest\""), "{contents}");
        assert!(contents.contains("\"message\":\"to a file\""), "{contents}");
    }

    #[cfg(feature = "otel")]
    #[tokio::test]
    async fn otel_layer_builds_from_config() {
        use super::super::config::TelemetryConfig;
        let config = LoggerConfig::new();
        let telemetry = TelemetryConfig::new("http://localhost:4317").service_name("test");
        // The exporter connects lazily, so building the layer succeeds without a
        // running collector.
        let built = otel_layer(&config, &telemetry);
        assert!(built.is_some());
        if let Some((_, provider)) = built {
            let _ = provider.shutdown();
        }
    }
}
