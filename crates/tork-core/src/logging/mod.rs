//! Structured, context-aware logging built on `tracing`.
//!
//! An injectable [`Logger`](crate::Logger) writes context-tagged records with a
//! fluent field API. In development the output is a colored, NestJS-style console;
//! in production it is one flat JSON object per line. HTTP requests, startup, and
//! the application lifecycle are logged automatically.

mod config;
mod event;
mod format;
mod logger;
mod span;
mod subscriber;

pub use config::{
    ErrorLogDetail, FileLogConfig, LogFormat, LoggerConfig, Rotation, TelemetryConfig,
};
pub use event::LogEvent;
pub use logger::Logger;
pub use span::LogSpan;
pub(crate) use subscriber::install;
