//! Core runtime for the Tork web framework.
//!
//! This crate holds everything that runs at request time: the HTTP server built
//! on Hyper and Tokio, the router, the dependency-injection traits, response and
//! error types. It contains no procedural macros; those live in `tork-macros`.
//!
//! End users do not depend on this crate directly. They depend on the `tork`
//! facade crate, which re-exports the public surface defined here.
#![forbid(unsafe_code)]

pub mod constants;

mod body;
mod error;
mod response;

pub use body::{ReqBody, RespBody};
pub use error::{Error, ErrorKind, Result};
pub use response::{IntoResponse, Json, Response, json_response};

// Commonly used `http` types are re-exported so users do not need to depend on
// the `http` crate directly.
pub use http::{Method, StatusCode};
