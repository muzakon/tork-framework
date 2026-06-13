//! The pluggable cache backend.

use std::time::Duration;

use crate::error::Result;
use crate::router::BoxFuture;

/// A cache backend that stores opaque byte values under string keys.
///
/// The [`Cache`](crate::Cache) handle serializes typed values to bytes and talks
/// to a store through this trait, so any backend (in-memory, Redis) works the same
/// from a handler's point of view. The trait is object-safe: a [`Cache`] holds an
/// `Arc<dyn CacheStore>`.
///
/// TTL convention: `None` keeps the entry until the store evicts it (no explicit
/// expiry); `Some(duration)` expires the entry after `duration`. A zero duration
/// is normalized to `None` ("never expire") by the [`Cache`](crate::Cache) handle
/// before it reaches the store.
pub trait CacheStore: Send + Sync + 'static {
    /// Returns the bytes stored under `key`, or `None` if absent or expired.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>>>;

    /// Stores `value` under `key`, expiring it after `ttl` when set.
    fn set(&self, key: String, value: Vec<u8>, ttl: Option<Duration>) -> BoxFuture<'_, Result<()>>;

    /// Removes the entry under `key`, if any.
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>>;

    /// Removes every entry from the store.
    fn clear(&self) -> BoxFuture<'_, Result<()>>;
}
