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

mod app;
mod body;
mod error;
mod extract;
mod openapi;
mod response;
mod router;
mod server;
mod service;
mod state;

pub use app::{App, AppInner};
pub use server::TorkService;
pub use body::{BoxError, ReqBody, RespBody, box_body};
pub use error::{Error, ErrorKind, Result};
pub use extract::{
    BearerToken, FromPathParam, FromRequest, PathParams, RequestContext, __extract_path_param,
};
pub use openapi::OpenApiProvider;
pub use response::{IntoResponse, Json, Response, json_response};
pub use router::matcher::{Match, Matcher};
pub use router::{BoxFuture, HandlerFn, Route, RouteMeta, Router};
pub use state::{AppStateRef, State, StateMap};

// Commonly used `http` types are re-exported so users do not need to depend on
// the `http` crate directly.
pub use http::{Method, StatusCode};
