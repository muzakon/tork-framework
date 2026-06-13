//! The pluggable counter backend for rate limiting.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::router::BoxFuture;

/// A backend that counts requests per key within a time window.
///
/// The throttler buckets each key by window and asks the store to count hits.
/// An in-memory store ([`MemoryThrottleStore`]) suits a single instance; a Redis
/// store (behind the `redis` feature) shares the count across instances.
pub trait ThrottleStore: Send + Sync + 'static {
    /// Atomically increments the counter at `key`, setting it to expire after
    /// `ttl` when first created, and returns the new count.
    fn incr(&self, key: String, ttl: Duration) -> BoxFuture<'_, Result<u64>>;
}

/// Number of entries past which a sweep of expired entries runs, bounding growth.
const SWEEP_THRESHOLD: usize = 4096;

/// One counter and the moment it expires.
struct Entry {
    count: u64,
    expires_at: Instant,
}

/// An in-memory [`ThrottleStore`] backed by a map of counters.
///
/// Counters expire lazily on access, and the whole map is swept once it grows
/// past a threshold, so a server with many distinct keys stays bounded.
#[derive(Clone, Default)]
pub struct MemoryThrottleStore {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

impl MemoryThrottleStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl ThrottleStore for MemoryThrottleStore {
    fn incr(&self, key: String, ttl: Duration) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let now = Instant::now();
            let mut map = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

            if map.len() > SWEEP_THRESHOLD {
                map.retain(|_, entry| entry.expires_at > now);
            }

            let entry = map.entry(key).or_insert(Entry {
                count: 0,
                expires_at: now + ttl,
            });
            // A new window starts when the previous one has expired.
            if entry.expires_at <= now {
                entry.count = 0;
                entry.expires_at = now + ttl;
            }
            entry.count += 1;
            Ok(entry.count)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_within_a_window_then_resets_after_it() {
        let store = MemoryThrottleStore::new();
        let ttl = Duration::from_millis(80);

        assert_eq!(store.incr("k".into(), ttl).await.unwrap(), 1);
        assert_eq!(store.incr("k".into(), ttl).await.unwrap(), 2);
        assert_eq!(store.incr("k".into(), ttl).await.unwrap(), 3);

        // After the window elapses the counter starts over.
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(store.incr("k".into(), ttl).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn distinct_keys_count_independently() {
        let store = MemoryThrottleStore::new();
        let ttl = Duration::from_secs(60);
        assert_eq!(store.incr("a".into(), ttl).await.unwrap(), 1);
        assert_eq!(store.incr("b".into(), ttl).await.unwrap(), 1);
        assert_eq!(store.incr("a".into(), ttl).await.unwrap(), 2);
    }
}
