//! A Redis-backed cache store (behind the `redis` feature).
//!
//! Sharing a cache across instances (so a value cached by one process is visible
//! to the others) needs an external store. [`RedisStore`] keeps entries in Redis,
//! using a multiplexed, auto-reconnecting connection. Keys are namespaced with a
//! prefix so [`clear`](RedisStore::clear) only touches this cache's keys, never the
//! whole database.

use std::time::Duration;

use redis::aio::ConnectionManager;

use crate::error::{Error, Result};
use crate::router::BoxFuture;

use super::store::CacheStore;

/// Default key prefix, keeping cache keys separate from other Redis data.
const DEFAULT_PREFIX: &str = "tork:";

/// A [`CacheStore`] backed by Redis.
///
/// Cloning is cheap: clones share one multiplexed connection. Build it with
/// [`connect`](RedisStore::connect), or use [`Cache::redis`](crate::Cache::redis).
#[derive(Clone)]
pub struct RedisStore {
    manager: ConnectionManager,
    prefix: String,
}

impl RedisStore {
    /// Connects to Redis at `url` (for example `redis://127.0.0.1:6379`) using the
    /// default key prefix.
    ///
    /// This opens its own connection. To share one connection with an injected
    /// [`Redis`](crate::Redis) (and a rate limiter, idempotency, ...), use
    /// [`from_redis`](RedisStore::from_redis) instead.
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with_prefix(url, DEFAULT_PREFIX).await
    }

    /// Connects to Redis at `url`, namespacing every key with `prefix`.
    pub async fn connect_with_prefix(url: &str, prefix: impl Into<String>) -> Result<Self> {
        let redis = crate::Redis::connect(url).await?;
        Ok(Self::from_manager(redis.connection(), prefix))
    }

    /// Builds a store over an existing [`Redis`](crate::Redis) connection, so the
    /// cache shares one connection pool with the rest of the application.
    pub fn from_redis(redis: &crate::Redis, prefix: impl Into<String>) -> Self {
        Self::from_manager(redis.connection(), prefix)
    }

    /// Builds a store over a raw connection manager.
    pub(crate) fn from_manager(manager: ConnectionManager, prefix: impl Into<String>) -> Self {
        Self {
            manager,
            prefix: prefix.into(),
        }
    }

    /// The default key prefix used by [`connect`](RedisStore::connect).
    pub(crate) fn default_prefix() -> &'static str {
        DEFAULT_PREFIX
    }

    fn full_key(&self, key: &str) -> String {
        prefixed(&self.prefix, key)
    }
}

impl CacheStore for RedisStore {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move {
            let mut conn = self.manager.clone();
            let value: Option<Vec<u8>> = redis::cmd("GET")
                .arg(self.full_key(key))
                .query_async(&mut conn)
                .await
                .map_err(redis_error)?;
            Ok(value)
        })
    }

    fn set(&self, key: String, value: Vec<u8>, ttl: Option<Duration>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut conn = self.manager.clone();
            let mut cmd = redis::cmd("SET");
            cmd.arg(self.full_key(&key)).arg(value);
            if let Some(ttl) = ttl {
                // PX is milliseconds and requires a positive value.
                let millis = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
                cmd.arg("PX").arg(millis);
            }
            cmd.query_async::<()>(&mut conn).await.map_err(redis_error)?;
            Ok(())
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut conn = self.manager.clone();
            redis::cmd("DEL")
                .arg(self.full_key(key))
                .query_async::<i64>(&mut conn)
                .await
                .map_err(redis_error)?;
            Ok(())
        })
    }

    fn clear(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut conn = self.manager.clone();
            let pattern = format!("{}*", self.prefix);
            // Scan and delete only this cache's keys, in batches; never FLUSHDB,
            // which would wipe unrelated data sharing the database.
            let mut cursor: u64 = 0;
            loop {
                let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(500)
                    .query_async(&mut conn)
                    .await
                    .map_err(redis_error)?;
                if !keys.is_empty() {
                    redis::cmd("DEL")
                        .arg(keys)
                        .query_async::<i64>(&mut conn)
                        .await
                        .map_err(redis_error)?;
                }
                if next == 0 {
                    break;
                }
                cursor = next;
            }
            Ok(())
        })
    }
}

/// Joins a key prefix and a key.
fn prefixed(prefix: &str, key: &str) -> String {
    format!("{prefix}{key}")
}

/// Maps a Redis driver error to an internal error.
fn redis_error(error: redis::RedisError) -> Error {
    Error::internal(format!("redis command failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_namespaced_with_the_prefix() {
        assert_eq!(prefixed("tork:", "user:1"), "tork:user:1");
        assert_eq!(prefixed("", "k"), "k");
    }

    #[tokio::test]
    async fn an_invalid_url_fails_to_connect() {
        // A bad scheme fails at URL parsing, without touching the network.
        let result = RedisStore::connect("http://not-redis").await;
        assert!(result.is_err());
    }
}
