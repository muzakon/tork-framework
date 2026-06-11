//! Installs the global `tracing` subscriber from a [`LoggerConfig`].

use std::io::IsTerminal;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use super::config::{LogFormat, LoggerConfig};
use super::format::{ConsoleFormat, JsonFormat, TorkFormat};

/// Keeps logging resources (such as the non-blocking writer worker) alive for the
/// lifetime of the application. Dropping it flushes and stops those workers.
// `install` and `LoggerHandle` are wired into `App` in a later commit of this phase.
#[allow(dead_code)]
#[must_use = "dropping the LoggerHandle stops background log workers"]
pub struct LoggerHandle {
    _guards: Vec<WorkerGuard>,
}

impl LoggerHandle {
    /// A handle that owns no background workers.
    #[allow(dead_code)]
    pub(crate) fn empty() -> Self {
        Self { _guards: Vec::new() }
    }
}

/// Installs the global subscriber from `config`.
///
/// Returns a [`LoggerHandle`] that must be kept alive while the application runs.
/// Installation is best-effort: if a global subscriber is already set (for example
/// in tests, or when the host application configured its own), this is a no-op.
#[allow(dead_code)]
pub(crate) fn install(config: &LoggerConfig) -> LoggerHandle {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let format = build_format(config);
    let mut guards = Vec::new();

    let installed = if config.non_blocking {
        let (writer, guard) = tracing_appender::non_blocking(std::io::stdout());
        guards.push(guard);
        let layer = tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(writer);
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init()
    } else {
        let layer = tracing_subscriber::fmt::layer()
            .event_format(format)
            .with_writer(std::io::stdout);
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init()
    };

    // A failure means a subscriber was already installed; that is fine.
    let _ = installed;
    LoggerHandle { _guards: guards }
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
