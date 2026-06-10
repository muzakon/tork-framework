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
pub mod middleware;
mod openapi;
mod response;
mod router;
mod server;
mod service;
mod state;

pub use app::{App, AppInner};
pub use middleware::{DuplicatePolicy, Middleware, Next, Request};
pub use server::TorkService;
pub use body::{BoxError, ReqBody, RespBody, box_body};
pub use error::{Error, ErrorDetail, ErrorKind, Result};
pub use extract::{
    BearerToken, FromPathParam, FromRequest, PathParams, RequestContext, Valid,
    __extract_path_param,
};
pub use openapi::OpenApiProvider;
pub use response::{
    IntoResponse, Json, Response, __finish, __finish_into, bytes_response, json_response,
};
pub use router::matcher::{Match, Matcher};
pub use router::{BoxFuture, HandlerFn, Route, RouteMeta, Router, SchemaThunk};
pub use state::{AppStateRef, State, StateMap};

// Commonly used `http` types are re-exported so users do not need to depend on
// the `http` crate directly.
pub use http::{Method, StatusCode};

/// Runtime support for the `#[tork::main]` macro.
///
/// This is generated-code support, not part of the user-facing API.
#[doc(hidden)]
pub mod __rt {
    /// Builds a multi-threaded Tokio runtime and blocks on `future` to completion.
    pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build the Tokio runtime")
            .block_on(future)
    }
}
