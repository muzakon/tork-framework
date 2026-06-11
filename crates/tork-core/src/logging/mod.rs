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
mod subscriber;

pub use config::{FileLogConfig, LogFormat, LoggerConfig, Rotation, TelemetryConfig};
pub use event::LogEvent;
pub use logger::Logger;
// Wired into `App` in a later commit of this phase.
#[allow(unused_imports)]
pub use subscriber::LoggerHandle;
#[allow(unused_imports)]
pub(crate) use subscriber::install;
