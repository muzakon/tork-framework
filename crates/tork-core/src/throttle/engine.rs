//! The throttle engine: policies, the runtime, and enforcement.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::header::RETRY_AFTER;
use http::HeaderValue;

use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};
use crate::response::{IntoResponse, Response};

use super::key::{ByIp, ThrottleKey};
use super::store::{MemoryThrottleStore, ThrottleStore};

/// The rate-limit policy attached to a route, emitted by the `throttle` attribute.
///
/// Construct-able in `const` context so the route macro can emit it directly.
#[derive(Clone, Copy, Debug)]
pub enum ThrottlePolicy {
    /// Use the application's global default policy, if one is configured.
    Inherit,
    /// Skip rate limiting for this route entirely.
    Skip,
    /// Use a globally-defined named policy.
    Named(&'static str),
    /// An inline limit of `limit` requests per `window_secs` seconds.
    Inline { limit: u32, window_secs: u64 },
}

/// A resolved limit: a number of requests per window.
#[derive(Clone, Copy)]
struct Limit {
    limit: u32,
    window: Duration,
}

/// Configures rate limiting; see [`App::throttle`](crate::App::throttle).
///
/// Define named policies, optionally mark one as the global default (applied to
/// every route that does not set its own), and choose the store.
pub struct Throttle {
    policies: HashMap<String, Limit>,
    default: Option<String>,
    store: Arc<dyn ThrottleStore>,
}

impl Throttle {
    /// Creates an empty configuration backed by an in-memory store.
    pub fn new() -> Self {
        Self {
            policies: HashMap::new(),
            default: None,
            store: Arc::new(MemoryThrottleStore::new()),
        }
    }

    /// Defines a named policy of `limit` requests per `window_secs` seconds.
    pub fn policy(mut self, name: &str, limit: u32, window_secs: u64) -> Self {
        self.policies.insert(
            name.to_owned(),
            Limit {
                limit,
                window: Duration::from_secs(window_secs.max(1)),
            },
        );
        self
    }

    /// Marks the named policy as the global default, applied to every route that
    /// does not declare its own `throttle`.
    pub fn default(mut self, name: &str) -> Self {
        self.default = Some(name.to_owned());
        self
    }

    /// Uses a custom counter store (the default is in-memory).
    pub fn store(mut self, store: impl ThrottleStore) -> Self {
        self.store = Arc::new(store);
        self
    }

    /// Uses a Redis store sharing the given connection, for distributed limiting.
    #[cfg(feature = "redis")]
    pub fn redis(mut self, redis: &crate::Redis) -> Self {
        self.store = Arc::new(super::redis::RedisThrottleStore::new(redis));
        self
    }
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

/// The runtime throttle engine, injectable and used by generated route code.
#[derive(Clone)]
pub struct Throttler {
    inner: Arc<Inner>,
}

struct Inner {
    policies: HashMap<String, Limit>,
    default: Option<Limit>,
    store: Arc<dyn ThrottleStore>,
}

/// The outcome of a rate-limit check.
enum Decision {
    Allow,
    Deny { retry_after: u64 },
}

impl Throttler {
    /// Builds the engine from its configuration.
    pub fn new(config: Throttle) -> Self {
        let default = config
            .default
            .as_ref()
            .and_then(|name| config.policies.get(name).copied());
        Self {
            inner: Arc::new(Inner {
                policies: config.policies,
                default,
                store: config.store,
            }),
        }
    }

    /// Resolves a policy to a concrete limit, or `None` when the route is not
    /// limited (skipped, or inherits with no default configured).
    fn resolve(&self, policy: &ThrottlePolicy) -> Option<Limit> {
        match policy {
            ThrottlePolicy::Skip => None,
            ThrottlePolicy::Inherit => self.inner.default,
            ThrottlePolicy::Inline { limit, window_secs } => Some(Limit {
                limit: *limit,
                window: Duration::from_secs((*window_secs).max(1)),
            }),
            ThrottlePolicy::Named(name) => self.inner.policies.get(*name).copied(),
        }
    }

    /// Counts a hit and decides whether the request is allowed.
    async fn decide(&self, scope: &str, limit: Limit, key: String) -> Decision {
        let window_secs = limit.window.as_secs().max(1);
        let now = unix_secs();
        let bucket = now / window_secs;
        let storage_key = format!("throttle:{scope}:{key}:{bucket}");

        match self.inner.store.incr(storage_key, limit.window).await {
            Ok(count) if count > u64::from(limit.limit) => Decision::Deny {
                // Seconds left in the current window.
                retry_after: window_secs - (now % window_secs),
            },
            // Allow on a store error too, so a backend hiccup fails open rather
            // than rejecting every request.
            _ => Decision::Allow,
        }
    }

    /// Enforces a policy, returning `Err(429)` when the limit is exceeded.
    ///
    /// `key` is the precomputed tracker; `None` falls back to the client IP.
    pub async fn check(
        &self,
        ctx: &RequestContext,
        policy: &ThrottlePolicy,
        key: Option<String>,
    ) -> Result<()> {
        let scope = ctx.uri().path().to_owned();
        match self.enforce(ctx, &scope, policy, key).await {
            Decision::Allow => Ok(()),
            Decision::Deny { .. } => Err(too_many()),
        }
    }

    /// Shared resolution: resolve the limit, compute the key, decide.
    async fn enforce(
        &self,
        ctx: &RequestContext,
        scope: &str,
        policy: &ThrottlePolicy,
        key: Option<String>,
    ) -> Decision {
        let limit = match self.resolve(policy) {
            Some(limit) => limit,
            None => return Decision::Allow,
        };
        let key = match key {
            Some(key) => key,
            None => match ByIp::throttle_key(ctx).await {
                Ok(key) => key,
                Err(_) => return Decision::Allow,
            },
        };
        self.decide(scope, limit, key).await
    }
}

impl FromRequest for Throttler {
    fn from_request(ctx: &RequestContext) -> impl Future<Output = Result<Self>> + Send {
        let resolved = ctx
            .state()
            .get::<Throttler>()
            .map(|throttler| (*throttler).clone())
            .ok_or_else(|| {
                Error::internal("throttling is not configured; call `App::throttle(...)`")
            });
        async move { resolved }
    }
}

/// Generated-code entry point: enforce a route's policy, returning a `429`
/// response when the limit is exceeded (or `None` to proceed).
///
/// A no-op (returns `None`) when no [`Throttler`] is configured, so the check the
/// route macro always emits costs only a state lookup in apps that do not throttle.
#[doc(hidden)]
pub async fn check_request(
    ctx: &RequestContext,
    scope: &'static str,
    policy: &ThrottlePolicy,
    key: Option<String>,
) -> Option<Response> {
    let throttler = ctx.state().get::<Throttler>()?;
    match throttler.enforce(ctx, scope, policy, key).await {
        Decision::Allow => None,
        Decision::Deny { retry_after } => Some(deny_response(retry_after)),
    }
}

/// Builds the `429 Too Many Requests` response with a `Retry-After` header.
fn deny_response(retry_after: u64) -> Response {
    let mut response = too_many().into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

/// The standard rate-limit error.
fn too_many() -> Error {
    Error::too_many_requests("rate limit exceeded").with_code("RATE_LIMITED")
}

/// Seconds since the Unix epoch.
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
