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

    // A failure means a subscriber was already installed; that is fine.
    let _ = Registry::default().with(layers).try_init();
    LoggerHandle { _guards: guards }
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
}
