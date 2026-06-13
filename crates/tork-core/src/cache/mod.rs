//! A small, store-agnostic cache with TTL.
//!
//! The cache is split in two: a [`Cache`] handle with a typed, ergonomic API
//! ([`get`](Cache::get)/[`set`](Cache::set)/[`get_or_set`](Cache::get_or_set)/...),
//! and a [`CacheStore`] backend that the handle talks to in raw bytes. Swapping
//! the backend (an in-memory store, a Redis store) does not change handler code.
//!
//! The default backend is [`MemoryStore`], an in-memory store with per-entry TTL.
//! Configure the cache on the application with
//! [`App::cache`](crate::App::cache); it is then injectable into handlers and
//! services as a [`Cache`] parameter, the same way a [`Logger`](crate::Logger) is.
//!
//! TTL follows the convention that a zero duration means "never expire".

mod handle;
mod memory;
#[cfg(feature = "redis")]
mod redis;
mod store;

pub use handle::Cache;
pub use memory::MemoryStore;
#[cfg(feature = "redis")]
pub use redis::RedisStore;
pub use store::CacheStore;
