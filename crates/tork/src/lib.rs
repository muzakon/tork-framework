//! Tork — a FastAPI-style backend web framework for Rust, built on Hyper and Tokio.
//!
//! This is the facade crate: the single crate end users depend on. It re-exports
//! the runtime from `tork-core`, the procedural macros from `tork-macros`, and,
//! when the `openapi` feature is enabled, the OpenAPI support from `tork-openapi`.
//!
//! # Example
//!
//! ```ignore
//! use tork::{App, OpenApi};
//!
//! #[tork::main]
//! async fn main() -> tork::Result<()> {
//!     App::new()
//!         .include_router(users::router())
//!         .openapi(OpenApi::new().title("My API").version("1.0.0").docs("/docs"))
//!         .serve("0.0.0.0:8000")
//!         .await
//! }
//! ```
#![forbid(unsafe_code)]

pub use tork_core::*;

// These globs are intentionally empty until later commits populate the macro and
// OpenAPI crates; the allow keeps intermediate builds warning-free.
#[allow(unused_imports)]
pub use tork_macros::*;

#[cfg(feature = "openapi")]
#[allow(unused_imports)]
pub use tork_openapi::*;
