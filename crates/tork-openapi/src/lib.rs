//! OpenAPI support for the Tork web framework.
//!
//! Provides the [`OpenApi`] builder used to configure the specification document.
//! It depends on `tork-core` to read the registered route table; `tork-core`
//! does not depend on this crate, so the dependency graph stays acyclic and
//! OpenAPI support can be turned off behind a feature flag in the facade crate.
#![forbid(unsafe_code)]

mod docs;
mod spec;

pub use spec::OpenApi;
