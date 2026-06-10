//! Trusted-host enforcement middleware.

use http::header::HOST;

use crate::error::{Error, Result};
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// A single allowed-host pattern.
enum HostPattern {
    /// Matches one host exactly.
    Exact(String),
    /// Matches any host ending with this `.suffix` (from a `*.suffix` pattern).
    Suffix(String),
}

/// Rejects requests whose `Host` header is not in an allow-list.
///
/// Patterns are exact (`example.com`) or single-leading-wildcard
/// (`*.example.com`, matching any subdomain). A non-matching host is rejected
/// with `400 Bad Request`.
pub struct TrustedHost {
    patterns: Vec<HostPattern>,
}

impl TrustedHost {
    /// Creates the middleware from an iterator of host patterns.
    pub fn new<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let patterns = hosts
            .into_iter()
            .map(|host| {
                let host = host.as_ref();
                match host.strip_prefix("*.") {
                    Some(rest) => HostPattern::Suffix(format!(".{}", rest.to_ascii_lowercase())),
                    None => HostPattern::Exact(host.to_ascii_lowercase()),
                }
            })
            .collect();
        Self { patterns }
    }

    /// Returns `true` if `host` matches any configured pattern.
    fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.patterns.iter().any(|pattern| match pattern {
            HostPattern::Exact(exact) => *exact == host,
            HostPattern::Suffix(suffix) => host.ends_with(suffix.as_str()),
        })
    }
}

impl Middleware for TrustedHost {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let host = request
            .headers()
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            // Strip any port so `example.com:8080` matches `example.com`.
            .map(|value| value.split(':').next().unwrap_or(value));

        let allowed = matches!(host, Some(host) if self.allows(host));
        if !allowed {
            return Box::pin(async { Err(Error::bad_request("invalid host header")) });
        }

        next.run(request)
    }

    fn name(&self) -> &'static str {
        "TrustedHost"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}
