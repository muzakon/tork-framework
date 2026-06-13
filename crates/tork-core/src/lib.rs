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
mod cache;
mod error;
mod extract;
mod hooks;
mod lifespan;
mod logging;
pub mod middleware;
mod multipart;
mod openapi;
mod realtime;
#[cfg(feature = "redis")]
mod redis_handle;
mod resources;
mod response;
mod router;
mod server;
mod service;
mod settings;
pub mod testing;
mod sse;
mod state;
mod ws;

pub use app::{App, AppInner, TestApp};
pub use lifespan::{Lifespan, LifespanContext, ReadyContext};
pub use logging::{
    FileLogConfig, LogEvent, LogFormat, LogSpan, Logger, LoggerConfig, Rotation, TelemetryConfig,
};
pub use middleware::{DuplicatePolicy, Middleware, Next, Request};
pub use multipart::{FileBytes, Form, FromMultipart, Multipart, UploadConfig, UploadFile};
#[doc(hidden)]
pub use multipart::{
    FileRule, MultipartForm, __parse_multipart, __validate_file_bytes, __validate_upload,
};
// Re-exported so handlers can name `Mime` without depending on the `mime` crate.
pub use mime;
pub use ipnet::IpNet;
pub use resources::Resources;
pub use server::TorkService;
pub use body::{BoxError, ReqBody, RespBody, box_body};
pub use cache::{Cache, CacheStore, MemoryStore};
#[cfg(feature = "redis")]
pub use cache::RedisStore;
#[cfg(feature = "redis")]
pub use redis_handle::Redis;
// Re-export the Redis client so applications use the same version for raw access
// (commands, Lua scripts, pipelines) without adding their own dependency.
#[cfg(feature = "redis")]
pub use ::redis;
pub use error::{Error, ErrorDetail, ErrorKind, Result};
pub use extract::{
    BearerToken, FromPathParam, FromRequest, LastEventId, PathParams, RequestContext, SseResume,
    Valid, __extract_path_param,
};
pub use hooks::{
    ErrorContext, ErrorEvent, PanicEvent, RequestEvent, ResponseEvent, ValidationErrorEvent,
};
pub use openapi::{AsyncApiProvider, OpenApiProvider};
pub use realtime::{Hub, Room};
// Re-exported so WebSocket handlers can name a subscription without depending on
// tokio directly.
pub use tokio::sync::broadcast::Receiver as WsReceiver;
pub use response::{
    IntoResponse, Json, Response, __finish, __finish_into, bytes_response, json_response,
};
pub use router::matcher::{Match, Matcher};
pub use router::{BoxFuture, HandlerFn, RequestBodyKind, Route, RouteMeta, Router, SchemaThunk};
pub use settings::{SecretString, SettingsLoader};
// Generated-code support for `#[derive(Inject)]` test overrides.
#[doc(hidden)]
pub use testing::__take_override;
pub use sse::{Sse, SseEvent};
pub use state::{AppStateRef, State, StateMap};
pub use ws::{
    WebSocket, WebSocketConfig, WebSocketConn, WsClose, WsCloseCode, WsConnectInfo,
    WsDisconnectInfo, WsError, WsMessage, __ws_handshake,
};

// Commonly used `http` types are re-exported so users do not need to depend on
// the `http` crate directly.
pub use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};

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

    /// Spawns a background task on the current Tokio runtime.
    ///
    /// Used by `#[websocket]` to drive a connection after the upgrade response is
    /// returned.
    pub fn spawn<F>(future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(future);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn runtime_block_on_executes_future() {
        let value = crate::__rt::block_on(async { 7usize });
        assert_eq!(value, 7);
    }
}
