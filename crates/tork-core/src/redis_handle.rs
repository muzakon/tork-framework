//! An injectable Redis connection (behind the `redis` feature).
//!
//! Redis is far more than a cache: idempotency keys, distributed locks, atomic
//! counters, Lua scripts, pub/sub. Tork does not wrap those — it manages the
//! connection and dependency injection, and hands you the raw [`redis`] client for
//! everything else. Register one with [`App::redis`](crate::App::redis) and inject
//! a [`Redis`] anywhere; reach for [`connection`](Redis::connection) (or the
//! re-exported [`redis`] crate's `Cmd`, `Script`, `pipe`, ...) to run any command.
//!
//! ```no_run
//! # use tork_core::Redis;
//! # async fn run(redis: Redis) -> tork_core::Result<()> {
//! // Idempotency: set only if absent.
//! let set: Option<String> = redis
//!     .query(redis::cmd("SET").arg("idem:abc").arg("1").arg("NX").arg("EX").arg(60))
//!     .await?;
//! let first_time = set.is_some();
//!
//! // A Lua script runs atomically on the server.
//! let count: i64 = redis::Script::new("return redis.call('INCR', KEYS[1])")
//!     .key("hits")
//!     .invoke_async(&mut redis.connection())
//!     .await
//!     .map_err(|e| tork_core::Error::internal(e.to_string()))?;
//! # let _ = (first_time, count);
//! # Ok(())
//! # }
//! ```

use redis::aio::ConnectionManager;

use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};

/// A cheap-to-clone handle to a Redis server.
///
/// Cloning shares one multiplexed, auto-reconnecting connection. Build it with
/// [`connect`](Redis::connect) and register it with
/// [`App::redis`](crate::App::redis) to make it injectable.
#[derive(Clone)]
pub struct Redis {
    manager: ConnectionManager,
}

impl Redis {
    /// Connects to Redis at `url` (for example `redis://127.0.0.1:6379`).
    pub async fn connect(url: &str) -> Result<Self> {
        let client = redis::Client::open(url)
            .map_err(|error| Error::internal(format!("invalid redis url: {error}")))?;
        let manager = client
            .get_connection_manager()
            .await
            .map_err(|error| Error::internal(format!("redis connection failed: {error}")))?;
        Ok(Self { manager })
    }

    /// Returns a cloned multiplexed connection for running any command directly.
    ///
    /// Use this as the escape hatch into the full [`redis`] API:
    /// `redis::cmd(...).query_async(&mut redis.connection())`, `redis::Script`,
    /// `redis::pipe`, and so on.
    pub fn connection(&self) -> ConnectionManager {
        self.manager.clone()
    }

    /// Runs a prepared command and decodes its reply, mapping driver errors.
    ///
    /// A thin convenience over [`connection`](Redis::connection):
    /// `redis.query(redis::cmd("GET").arg(key)).await`.
    pub async fn query<T: redis::FromRedisValue>(&self, command: &redis::Cmd) -> Result<T> {
        let mut conn = self.manager.clone();
        command
            .query_async(&mut conn)
            .await
            .map_err(|error| Error::internal(format!("redis command failed: {error}")))
    }
}

impl FromRequest for Redis {
    fn from_request(ctx: &RequestContext) -> impl std::future::Future<Output = Result<Self>> + Send {
        let resolved = ctx
            .state()
            .get::<Redis>()
            .map(|redis| (*redis).clone())
            .ok_or_else(|| {
                Error::internal("redis is not configured; call `App::redis(...)` to enable it")
            });
        async move { resolved }
    }
}
