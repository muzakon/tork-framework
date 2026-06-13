//! The default in-memory cache store.

use std::time::{Duration, Instant};

use moka::future::Cache as MokaCache;
use moka::Expiry;

use crate::error::Result;
use crate::router::BoxFuture;

use super::store::CacheStore;

/// Default maximum number of entries before the least-used are evicted.
const DEFAULT_MAX_CAPACITY: u64 = 10_000;

/// A stored value together with its optional time-to-live.
#[derive(Clone)]
struct Entry {
    data: Vec<u8>,
    ttl: Option<Duration>,
}

/// Expires each entry after its own TTL (`None` means no time-based expiry, so the
/// entry stays until it is evicted by the capacity limit).
struct PerEntryTtl;

impl Expiry<String, Entry> for PerEntryTtl {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &Entry,
        _created_at: Instant,
    ) -> Option<Duration> {
        value.ttl
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &Entry,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        // Re-setting a key adopts the new value's TTL.
        value.ttl
    }
}

/// An in-memory [`CacheStore`] with per-entry TTL, backed by `moka`.
///
/// Entries expire after their TTL and the least-recently-used entries are evicted
/// once the capacity limit is reached, so the cache stays bounded.
#[derive(Clone)]
pub struct MemoryStore {
    inner: MokaCache<String, Entry>,
}

impl MemoryStore {
    /// Creates a store holding up to [`DEFAULT_MAX_CAPACITY`] entries.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_CAPACITY)
    }

    /// Creates a store holding up to `max_capacity` entries.
    pub fn with_capacity(max_capacity: u64) -> Self {
        let inner = MokaCache::builder()
            .max_capacity(max_capacity)
            .expire_after(PerEntryTtl)
            .build();
        Self { inner }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheStore for MemoryStore {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>>> {
        Box::pin(async move { Ok(self.inner.get(key).await.map(|entry| entry.data)) })
    }

    fn set(&self, key: String, value: Vec<u8>, ttl: Option<Duration>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.inner.insert(key, Entry { data: value, ttl }).await;
            Ok(())
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.inner.invalidate(key).await;
            Ok(())
        })
    }

    fn clear(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.inner.invalidate_all();
            Ok(())
        })
    }
}
