//! Security-headers middleware.

use http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use http::{HeaderMap, HeaderName, HeaderValue};

use crate::error::Result;
use crate::middleware::{DuplicatePolicy, Middleware, Next, Request};
use crate::response::Response;
use crate::router::BoxFuture;

/// Default `Strict-Transport-Security`: one year, including subdomains.
const DEFAULT_HSTS: &str = "max-age=31536000; includeSubDomains";

/// Adds a baseline of security-related response headers.
///
/// Every response that does not already carry the header gets, by default:
///
/// - `Strict-Transport-Security: max-age=31536000; includeSubDomains` — tells
///   browsers to use HTTPS for a year (ignored by browsers over plain HTTP, so
///   it is safe to send unconditionally behind a TLS-terminating proxy).
/// - `X-Content-Type-Options: nosniff` — stop MIME sniffing.
/// - `X-Frame-Options: DENY` — block framing (clickjacking).
/// - `Referrer-Policy: no-referrer` — do not leak the URL on outbound requests.
///
/// A `Content-Security-Policy` is application-specific and off by default; set
/// one with [`content_security_policy`](SecurityHeaders::content_security_policy).
/// Each header is only added when the response does not already set it, so a
/// handler can intentionally override any of them.
///
/// # Examples
///
/// ```
/// use tork_core::middleware::SecurityHeaders;
///
/// // Secure defaults.
/// let _ = SecurityHeaders::new();
///
/// // Allow same-origin framing and add a strict CSP.
/// let _ = SecurityHeaders::new()
///     .frame_options("SAMEORIGIN")
///     .content_security_policy("default-src 'self'");
/// ```
pub struct SecurityHeaders {
    hsts: Option<HeaderValue>,
    content_type_options: Option<HeaderValue>,
    frame_options: Option<HeaderValue>,
    referrer_policy: Option<HeaderValue>,
    content_security_policy: Option<HeaderValue>,
}

impl SecurityHeaders {
    /// Creates the middleware with secure defaults.
    pub fn new() -> Self {
        Self {
            hsts: Some(HeaderValue::from_static(DEFAULT_HSTS)),
            content_type_options: Some(HeaderValue::from_static("nosniff")),
            frame_options: Some(HeaderValue::from_static("DENY")),
            referrer_policy: Some(HeaderValue::from_static("no-referrer")),
            content_security_policy: None,
        }
    }

    /// Sets the full `Strict-Transport-Security` value.
    pub fn hsts(mut self, value: &'static str) -> Self {
        self.hsts = Some(HeaderValue::from_static(value));
        self
    }

    /// Sets `Strict-Transport-Security` from a `max-age` in seconds, optionally
    /// applying it to subdomains.
    pub fn hsts_max_age(mut self, seconds: u64, include_subdomains: bool) -> Self {
        let value = if include_subdomains {
            format!("max-age={seconds}; includeSubDomains")
        } else {
            format!("max-age={seconds}")
        };
        // The value is composed from a number and fixed ASCII, so it is valid.
        self.hsts = HeaderValue::from_str(&value).ok();
        self
    }

    /// Stops sending `Strict-Transport-Security` (for example for a plain-HTTP
    /// service that should not advertise HSTS).
    pub fn without_hsts(mut self) -> Self {
        self.hsts = None;
        self
    }

    /// Sets the `X-Frame-Options` value (for example `SAMEORIGIN`).
    pub fn frame_options(mut self, value: &'static str) -> Self {
        self.frame_options = Some(HeaderValue::from_static(value));
        self
    }

    /// Stops sending `X-Frame-Options`.
    pub fn without_frame_options(mut self) -> Self {
        self.frame_options = None;
        self
    }

    /// Sets the `Referrer-Policy` value.
    pub fn referrer_policy(mut self, value: &'static str) -> Self {
        self.referrer_policy = Some(HeaderValue::from_static(value));
        self
    }

    /// Stops sending `Referrer-Policy`.
    pub fn without_referrer_policy(mut self) -> Self {
        self.referrer_policy = None;
        self
    }

    /// Stops sending `X-Content-Type-Options`.
    pub fn without_content_type_options(mut self) -> Self {
        self.content_type_options = None;
        self
    }

    /// Sets the `Content-Security-Policy` value (off by default).
    pub fn content_security_policy(mut self, value: &'static str) -> Self {
        self.content_security_policy = Some(HeaderValue::from_static(value));
        self
    }
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        Self::new()
    }
}

/// Inserts `value` under `name` only when the response does not already set it,
/// so a handler can override any individual header.
fn set_if_absent(headers: &mut HeaderMap, name: HeaderName, value: &Option<HeaderValue>) {
    if let Some(value) = value {
        if !headers.contains_key(&name) {
            headers.insert(name, value.clone());
        }
    }
}

impl Middleware for SecurityHeaders {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        let hsts = self.hsts.clone();
        let content_type_options = self.content_type_options.clone();
        let frame_options = self.frame_options.clone();
        let referrer_policy = self.referrer_policy.clone();
        let content_security_policy = self.content_security_policy.clone();

        Box::pin(async move {
            let mut response = next.run(request).await?;
            let headers = response.headers_mut();
            set_if_absent(headers, STRICT_TRANSPORT_SECURITY, &hsts);
            set_if_absent(headers, X_CONTENT_TYPE_OPTIONS, &content_type_options);
            set_if_absent(headers, X_FRAME_OPTIONS, &frame_options);
            set_if_absent(headers, REFERRER_POLICY, &referrer_policy);
            set_if_absent(headers, CONTENT_SECURITY_POLICY, &content_security_policy);
            Ok(response)
        })
    }

    fn name(&self) -> &'static str {
        "SecurityHeaders"
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_set_the_baseline_headers() {
        let mw = SecurityHeaders::new();
        assert_eq!(mw.hsts.as_ref().unwrap(), DEFAULT_HSTS);
        assert_eq!(mw.frame_options.as_ref().unwrap(), "DENY");
        assert_eq!(mw.content_type_options.as_ref().unwrap(), "nosniff");
        assert_eq!(mw.referrer_policy.as_ref().unwrap(), "no-referrer");
        assert!(mw.content_security_policy.is_none());
    }

    #[test]
    fn builders_customize_and_disable_headers() {
        let mw = SecurityHeaders::new()
            .hsts_max_age(60, false)
            .frame_options("SAMEORIGIN")
            .content_security_policy("default-src 'self'")
            .without_referrer_policy();
        assert_eq!(mw.hsts.as_ref().unwrap(), "max-age=60");
        assert_eq!(mw.frame_options.as_ref().unwrap(), "SAMEORIGIN");
        assert_eq!(
            mw.content_security_policy.as_ref().unwrap(),
            "default-src 'self'"
        );
        assert!(mw.referrer_policy.is_none());
    }
}
