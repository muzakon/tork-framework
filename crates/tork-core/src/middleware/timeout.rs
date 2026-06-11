//! Request-timeout middleware.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Fails a request with `504 Gateway Timeout` if it runs longer than a deadline.
pub struct Timeout {
    duration: Duration,
}

impl Timeout {
    /// Creates a timeout from a [`Duration`].
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }

    /// Creates a timeout of `secs` seconds.
    pub fn seconds(secs: u64) -> Self {
        Self::new(Duration::from_secs(secs))
    }

    /// Creates a timeout of `millis` milliseconds.
    pub fn millis(millis: u64) -> Self {
        Self::new(Duration::from_millis(millis))
    }
}

impl Middleware for Timeout {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let duration = self.duration;
        Box::pin(async move {
            match tokio::time::timeout(duration, next.run(request)).await {
                Ok(result) => result,
                Err(_elapsed) => Err(Error::gateway_timeout("request timed out")),
            }
        })
    }

    fn name(&self) -> &'static str {
        "Timeout"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_preserve_requested_duration() {
        assert_eq!(Timeout::new(Duration::from_secs(3)).duration, Duration::from_secs(3));
        assert_eq!(Timeout::seconds(2).duration, Duration::from_secs(2));
        assert_eq!(Timeout::millis(25).duration, Duration::from_millis(25));
    }
}
