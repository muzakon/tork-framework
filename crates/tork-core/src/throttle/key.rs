//! Rate-limit keys: how a request is identified for counting.

use std::future::Future;

use crate::error::Result;
use crate::extract::RequestContext;

/// Produces the key a request is rate-limited by (the "tracker").
///
/// The default is the client IP ([`ByIp`]). Implement this on a unit type to key
/// by something else — a user id, an API key, a tenant — reusing dependency
/// injection inside, since it has the full [`RequestContext`]:
///
/// ```ignore
/// struct ByUser;
/// impl ThrottleKey for ByUser {
///     async fn throttle_key(ctx: &RequestContext) -> tork::Result<String> {
///         Ok(CurrentUser::from_request(ctx).await?.id.to_string())
///     }
/// }
/// ```
pub trait ThrottleKey: Send + Sync + 'static {
    /// Returns the key for `ctx`.
    fn throttle_key(ctx: &RequestContext) -> impl Future<Output = Result<String>> + Send;
}

/// The default key strategy: the client IP address.
///
/// Falls back to `"unknown"` when no peer address is available (for example an
/// in-process test request), so such requests share one counter rather than going
/// unlimited.
pub struct ByIp;

impl ThrottleKey for ByIp {
    fn throttle_key(ctx: &RequestContext) -> impl Future<Output = Result<String>> + Send {
        let key = ctx
            .peer_addr()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        async move { Ok(key) }
    }
}
