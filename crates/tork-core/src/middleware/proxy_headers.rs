//! Proxy-header normalization middleware.

use std::net::IpAddr;

use http::header::HOST;
use ipnet::IpNet;
use tracing::warn;

use crate::extract::{RequestScheme, peer_addr_from_extensions};
use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Header conveying the original host through a terminating proxy.
const FORWARDED_HOST: &str = "x-forwarded-host";
/// Header conveying the original scheme through a terminating proxy.
const FORWARDED_PROTO: &str = "x-forwarded-proto";

/// Normalizes proxy-forwarded headers onto the request.
///
/// When the request comes through a terminating proxy, the original host arrives
/// in `X-Forwarded-Host`; this middleware rewrites the `Host` header from it so
/// downstream host-based middleware (such as [`TrustedHost`](super::TrustedHost))
/// sees the client-facing host. Register it before those middlewares. The
/// forwarded scheme is honored directly by
/// [`HttpsRedirect`](super::HttpsRedirect).
pub struct ProxyHeaders {
    trusted_ips: Vec<IpAddr>,
    trusted_cidrs: Vec<IpNet>,
}

impl ProxyHeaders {
    /// Creates the middleware.
    pub fn new() -> Self {
        Self {
            trusted_ips: Vec::new(),
            trusted_cidrs: Vec::new(),
        }
    }

    /// Trusts a single reverse proxy address.
    pub fn trust_proxy(mut self, addr: IpAddr) -> Self {
        self.trusted_ips.push(addr);
        self
    }

    /// Trusts a reverse-proxy network.
    pub fn trust_cidr(mut self, network: IpNet) -> Self {
        self.trusted_cidrs.push(network);
        self
    }

    /// Trusts loopback reverse proxies (`127.0.0.1` and `::1`).
    pub fn trust_loopback(self) -> Self {
        self.trust_proxy(IpAddr::from([127, 0, 0, 1]))
            .trust_proxy(IpAddr::from(std::net::Ipv6Addr::LOCALHOST))
    }

    fn is_trusted(&self, request: &Request) -> bool {
        let Some(peer) = peer_addr_from_extensions(request.extensions()) else {
            return false;
        };
        self.trusted_ips.iter().any(|addr| *addr == peer.ip())
            || self.trusted_cidrs.iter().any(|network| network.contains(&peer.ip()))
    }

    fn forwarded_value<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

impl Default for ProxyHeaders {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for ProxyHeaders {
    fn handle(&self, mut request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        if !self.is_trusted(&request) {
            return next.run(request);
        }

        if let Some(forwarded_host) = Self::forwarded_value(&request, FORWARDED_HOST) {
            if let Ok(value) = http::HeaderValue::from_str(forwarded_host) {
                request.headers_mut().insert(HOST, value);
            }
        }

        if let Some(forwarded_proto) = Self::forwarded_value(&request, FORWARDED_PROTO) {
            let scheme = if forwarded_proto.eq_ignore_ascii_case("https") {
                Some(RequestScheme::Https)
            } else if forwarded_proto.eq_ignore_ascii_case("http") {
                Some(RequestScheme::Http)
            } else {
                None
            };

            if let Some(scheme) = scheme {
                request.extensions_mut().insert(scheme);
            } else {
                warn!("tork: ignoring unsupported X-Forwarded-Proto value");
            }
        }
        next.run(request)
    }

    fn name(&self) -> &'static str {
        "ProxyHeaders"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_metadata_is_stable() {
        let middleware = ProxyHeaders::new();
        assert_eq!(middleware.name(), "ProxyHeaders");
        assert_eq!(middleware.duplicate_policy(), DuplicatePolicy::Reject);
    }

    #[test]
    fn default_impl_uses_new() {
        // Default must produce the same value as new().
        let middleware: ProxyHeaders = Default::default();
        assert!(middleware.trusted_ips.is_empty());
        assert!(middleware.trusted_cidrs.is_empty());
    }

    #[test]
    fn trust_builders_register_expected_networks() {
        let middleware = ProxyHeaders::new()
            .trust_proxy(IpAddr::from([10, 0, 0, 1]))
            .trust_cidr("10.0.0.0/24".parse().unwrap())
            .trust_loopback();

        assert!(middleware.trusted_ips.contains(&IpAddr::from([10, 0, 0, 1])));
        assert!(middleware
            .trusted_cidrs
            .contains(&"10.0.0.0/24".parse().unwrap()));
        assert!(middleware
            .trusted_ips
            .contains(&IpAddr::from([127, 0, 0, 1])));
    }
}
