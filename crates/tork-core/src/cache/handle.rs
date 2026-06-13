//! The typed cache handle.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};

use super::memory::MemoryStore;
use super::store::CacheStore;

/// A cheap-to-clone handle over a [`CacheStore`], with a typed, ergonomic API.
///
/// Values are serialized to JSON before being stored and deserialized on the way
/// out, so any `serde` type can be cached. Cloning a `Cache` is cheap (it shares
/// the underlying store), so it is held as an injected resource.
///
/// # Examples
///
/// ```no_run
/// # use tork_core::Cache;
/// # async fn run(cache: Cache) -> tork_core::Result<()> {
/// cache.set("greeting", &"hello").await?;
/// let greeting: Option<String> = cache.get("greeting").await?;
/// assert_eq!(greeting.as_deref(), Some("hello"));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Cache {
    store: Arc<dyn CacheStore>,
    default_ttl: Option<Duration>,
}

impl Cache {
    /// Builds a cache over a custom [`CacheStore`].
    pub fn new(store: impl CacheStore) -> Self {
        Self {
            store: Arc::new(store),
            default_ttl: None,
        }
    }

    /// Builds a cache over the default in-memory store ([`MemoryStore`]).
    pub fn in_memory() -> Self {
        Self::new(MemoryStore::new())
    }

    /// Sets the TTL applied by [`set`](Cache::set) when no explicit TTL is given.
    ///
    /// Without this, `set` stores entries with no expiry (they live until evicted
    /// by the store's capacity limit).
    pub fn default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = normalize_ttl(Some(ttl));
        self
    }

    /// Returns the value stored under `key`, or `None` if absent or expired.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.store.get(key).await? {
            Some(bytes) => {
                let value = serde_json::from_slice(&bytes).map_err(|error| {
                    Error::internal(format!("cache value could not be deserialized: {error}"))
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Stores `value` under `key` using the cache's default TTL.
    pub async fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.write(key, value, self.default_ttl).await
    }

    /// Stores `value` under `key`, expiring it after `ttl`.
    ///
    /// A zero `ttl` means the entry never expires.
    pub async fn set_ttl<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) -> Result<()> {
        self.write(key, value, normalize_ttl(Some(ttl))).await
    }

    /// Returns the cached value under `key`, or computes it with `init`, stores it
    /// (with `ttl`, falling back to the default TTL), and returns it.
    ///
    /// This is the cache-aside pattern in one call: a hit returns immediately
    /// without running `init`; a miss runs `init` once and caches the result.
    pub async fn get_or_set<T, F, Fut>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        init: F,
    ) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if let Some(found) = self.get::<T>(key).await? {
            return Ok(found);
        }
        let value = init().await?;
        let ttl = match ttl {
            Some(ttl) => normalize_ttl(Some(ttl)),
            None => self.default_ttl,
        };
        self.write(key, &value, ttl).await?;
        Ok(value)
    }

    /// Removes the entry under `key`, if any.
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(key).await
    }

    /// Removes every entry from the cache.
    pub async fn clear(&self) -> Result<()> {
        self.store.clear().await
    }

    /// Serializes `value` and writes it to the store.
    async fn write<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|error| {
            Error::internal(format!("cache value could not be serialized: {error}"))
        })?;
        self.store.set(key.to_owned(), bytes, ttl).await
    }
}

/// Normalizes a TTL: a zero duration means "never expire" (`None`).
fn normalize_ttl(ttl: Option<Duration>) -> Option<Duration> {
    match ttl {
        Some(ttl) if ttl.is_zero() => None,
        other => other,
    }
}

impl FromRequest for Cache {
    fn from_request(ctx: &RequestContext) -> impl Future<Output = Result<Self>> + Send {
        let resolved = ctx
            .state()
            .get::<Cache>()
            .map(|cache| (*cache).clone())
            .ok_or_else(|| {
                Error::internal(
                    "cache is not configured; call `App::cache(...)` to enable it",
                )
            });
        async move { resolved }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct User {
        id: i64,
        name: String,
    }

    #[tokio::test]
    async fn round_trips_a_typed_value() {
        let cache = Cache::in_memory();
        let user = User {
            id: 1,
            name: "alice".into(),
        };
        cache.set("user:1", &user).await.unwrap();

        let stored: Option<User> = cache.get("user:1").await.unwrap();
        assert_eq!(stored, Some(user));
    }

    #[tokio::test]
    async fn a_missing_key_is_none() {
        let cache = Cache::in_memory();
        let stored: Option<String> = cache.get("absent").await.unwrap();
        assert_eq!(stored, None);
    }

    #[tokio::test]
    async fn an_entry_expires_after_its_ttl() {
        let cache = Cache::in_memory();
        cache
            .set_ttl("k", &"v", Duration::from_millis(50))
            .await
            .unwrap();

        assert_eq!(cache.get::<String>("k").await.unwrap().as_deref(), Some("v"));
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(cache.get::<String>("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_zero_ttl_never_expires() {
        let cache = Cache::in_memory();
        cache.set_ttl("k", &"v", Duration::ZERO).await.unwrap();

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(cache.get::<String>("k").await.unwrap().as_deref(), Some("v"));
    }

    #[tokio::test]
    async fn default_ttl_applies_to_plain_set() {
        let cache = Cache::in_memory().default_ttl(Duration::from_millis(50));
        cache.set("k", &"v").await.unwrap();

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(cache.get::<String>("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_or_set_computes_once_then_hits_the_cache() {
        let cache = Cache::in_memory();
        let calls = AtomicUsize::new(0);

        let compute = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Error>(User {
                id: 7,
                name: "bob".into(),
            })
        };

        let first = cache.get_or_set("user:7", None, compute).await.unwrap();
        let second = cache.get_or_set("user:7", None, compute).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "init runs only on a miss");
    }

    #[tokio::test]
    async fn delete_and_clear_remove_entries() {
        let cache = Cache::in_memory();
        cache.set("a", &1).await.unwrap();
        cache.set("b", &2).await.unwrap();

        cache.delete("a").await.unwrap();
        assert_eq!(cache.get::<i32>("a").await.unwrap(), None);
        assert_eq!(cache.get::<i32>("b").await.unwrap(), Some(2));

        cache.clear().await.unwrap();
        assert_eq!(cache.get::<i32>("b").await.unwrap(), None);
    }
}
