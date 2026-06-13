//! A Redis-backed throttle store for distributed rate limiting (feature `redis`).

use std::time::Duration;

use crate::error::{Error, Result};
use crate::router::BoxFuture;
use crate::Redis;

use super::store::ThrottleStore;

/// A [`ThrottleStore`] that keeps counters in Redis, so several instances share
/// one limit. Built with [`Throttle::redis`](super::Throttle::redis); reuses the
/// injected [`Redis`] connection.
#[derive(Clone)]
pub struct RedisThrottleStore {
    redis: Redis,
}

impl RedisThrottleStore {
    pub(crate) fn new(redis: &Redis) -> Self {
        Self {
            redis: redis.clone(),
        }
    }
}

impl ThrottleStore for RedisThrottleStore {
    fn incr(&self, key: String, ttl: Duration) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let ttl_secs = ttl.as_secs().max(1);
            // Fixed window, atomic on the server: increment, and set the expiry
            // only on the first hit so the window does not keep sliding.
            let script = ::redis::Script::new(
                "local c = redis.call('INCR', KEYS[1]) \
                 if c == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end \
                 return c",
            );
            let mut conn = self.redis.connection();
            let count: i64 = script
                .key(key)
                .arg(ttl_secs)
                .invoke_async(&mut conn)
                .await
                .map_err(|error| Error::internal(format!("redis throttle failed: {error}")))?;
            Ok(count.max(0) as u64)
        })
    }

    fn count(&self, key: String) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let mut conn = self.redis.connection();
            let count: Option<i64> = ::redis::cmd("GET")
                .arg(key)
                .query_async(&mut conn)
                .await
                .map_err(|error| Error::internal(format!("redis throttle failed: {error}")))?;
            Ok(count.unwrap_or(0).max(0) as u64)
        })
    }
}
