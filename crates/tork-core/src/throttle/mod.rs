//! Rate limiting: a NestJS-style throttler with pluggable storage.
//!
//! Define policies on the app with [`App::throttle`](crate::App::throttle), then
//! apply them with the `throttle` attribute on routers and routes — a named policy
//! (`throttle = "default"`), an inline one (`throttle(limit = 3, ttl = 60)`), or
//! `throttle = "skip"` to bypass. An endpoint's policy overrides its router's,
//! which overrides the global default. Requests are counted per route and per
//! [key](ThrottleKey) (the client IP by default); exceeding the limit returns
//! `429 Too Many Requests` with a `Retry-After` header.

mod engine;
mod key;
#[cfg(feature = "redis")]
mod redis;
mod store;

pub use engine::{Throttle, ThrottlePolicy, Throttler};
pub use key::{ByIp, ThrottleKey};
pub use store::{MemoryThrottleStore, ThrottleStore};

#[cfg(feature = "redis")]
pub use redis::RedisThrottleStore;

#[doc(hidden)]
pub use engine::check_request;
