//! A minimal cookie jar for the test client.
//!
//! It stores each cookie as a `name=value` pair: `Set-Cookie` response headers are
//! recorded and sent back on subsequent requests. Cookie attributes (path, expiry,
//! flags) are not interpreted; this is enough to carry a session through a test.

use std::collections::HashMap;

use http::header::{COOKIE, SET_COOKIE};
use http::HeaderMap;

/// Stored cookies, keyed by name.
#[derive(Default, Clone)]
pub(crate) struct CookieJar {
    entries: HashMap<String, String>,
}

impl CookieJar {
    /// Sets a cookie directly (used for seeding from the builder).
    pub(crate) fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.insert(name.into(), value.into());
    }

    /// Records every `Set-Cookie` header from a response.
    pub(crate) fn store(&mut self, headers: &HeaderMap) {
        for value in headers.get_all(SET_COOKIE).iter() {
            let Ok(text) = value.to_str() else {
                continue;
            };
            // Keep only the `name=value` pair before the first attribute.
            let pair = text.split(';').next().unwrap_or(text).trim();
            if let Some((name, value)) = pair.split_once('=') {
                self.entries
                    .insert(name.trim().to_owned(), value.trim().to_owned());
            }
        }
    }

    /// Builds the `Cookie` request header value, if any cookies are stored.
    pub(crate) fn header_value(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let mut pairs: Vec<String> = self
            .entries
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        // A stable order keeps the produced header deterministic across requests.
        pairs.sort();
        Some(pairs.join("; "))
    }

    /// Applies the stored cookies to a request's headers.
    pub(crate) fn apply(&self, headers: &mut HeaderMap) {
        if let Some(value) = self.header_value() {
            if let Ok(value) = value.parse() {
                headers.insert(COOKIE, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn store_overwrites_values_and_sorts_header_output() {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("session=one; Path=/"));
        headers.append(SET_COOKIE, HeaderValue::from_static("theme=dark; HttpOnly"));
        headers.append(SET_COOKIE, HeaderValue::from_static("session=two; Secure"));

        let mut jar = CookieJar::default();
        jar.store(&headers);

        assert_eq!(
            jar.header_value().as_deref(),
            Some("session=two; theme=dark")
        );
    }

    #[test]
    fn apply_uses_seeded_cookie_entries() {
        let mut jar = CookieJar::default();
        jar.set("token", "abc");
        jar.set("mode", "test");

        let mut headers = HeaderMap::new();
        jar.apply(&mut headers);

        assert_eq!(headers[COOKIE], "mode=test; token=abc");
    }
}
