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
mod env;
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
pub mod security;
mod server;
#[cfg(feature = "tls")]
pub mod tls;
mod service;
mod settings;
mod sse;
mod state;
pub mod testing;
mod throttle;
mod ws;

pub use app::{App, AppInner, TestApp};
pub use lifespan::{Lifespan, LifespanContext, ReadyContext};
pub use logging::{
    ErrorLogDetail, FileLogConfig, LogEvent, LogFormat, LogSpan, Logger, LoggerConfig, Rotation,
    TelemetryConfig,
};
pub use middleware::{DuplicatePolicy, Middleware, Next, Request};
#[doc(hidden)]
pub use multipart::{
    __parse_multipart, __validate_file_bytes, __validate_upload, FileRule, MultipartForm,
};
pub use multipart::{FileBytes, Form, FromMultipart, Multipart, UploadConfig, UploadFile};
// Re-exported so handlers can name `Mime` without depending on the `mime` crate.
pub use body::{box_body, BoxError, ReqBody, RespBody};
#[cfg(feature = "redis")]
pub use cache::RedisStore;
pub use cache::{Cache, CacheStore, MemoryStore};
pub use ipnet::IpNet;
pub use mime;
#[cfg(feature = "redis")]
pub use redis_handle::Redis;
pub use resources::Resources;
pub use server::{Http1Config, Http2Config, TorkService};
#[doc(hidden)]
pub use throttle::check_request as __throttle_check;
#[cfg(feature = "redis")]
pub use throttle::RedisThrottleStore;
pub use throttle::{
    ByIp, MemoryThrottleStore, Throttle, ThrottleKey, ThrottlePolicy, ThrottleStore, Throttler,
};
// Re-export the Redis client so applications use the same version for raw access
// (commands, Lua scripts, pipelines) without adding their own dependency.
#[cfg(feature = "redis")]
pub use ::redis;
pub use error::{Error, ErrorDetail, ErrorKind, Result};
pub use extract::{
    __extract_path_param, BearerToken, FromPathParam, FromRequest, LastEventId, PathParams,
    RequestContext, SseResume, Valid,
};
pub use hooks::{
    ErrorContext, ErrorEvent, PanicEvent, RequestEvent, ResponseEvent, ValidationErrorEvent,
};
pub use openapi::{AsyncApiProvider, OpenApiProvider};
pub use realtime::{Hub, Room};
// Re-exported so WebSocket handlers can name a subscription without depending on
// tokio directly.
pub use response::{
    __finish, __finish_into, bytes_response, json_response, IntoResponse, Json, Response,
};
pub use router::matcher::{Match, Matcher};
pub use router::{BoxFuture, HandlerFn, RequestBodyKind, Route, RouteMeta, Router, SchemaThunk};
pub use settings::{SecretString, SettingsLoader};
pub use security::constant_time_eq;
#[cfg(feature = "tls")]
pub use tls::TlsConfig;
pub use tokio::sync::broadcast::Receiver as WsReceiver;
// Generated-code support for `#[derive(Inject)]` test overrides.
#[doc(hidden)]
pub use sse::__sse_into_response;
pub use sse::{Sse, SseEvent};
pub use state::{AppStateRef, State, StateMap};
#[doc(hidden)]
pub use testing::__take_override;
pub use ws::{
    __ws_handshake, WebSocket, WebSocketConfig, WebSocketConn, WsClose, WsCloseCode, WsConnectInfo,
    WsDisconnectInfo, WsError, WsMessage,
};

// Commonly used `http` types are re-exported so users do not need to depend on
// the `http` crate directly.
pub use http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};

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
