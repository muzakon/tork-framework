//! Header-based extractors.

use http::header::{AUTHORIZATION, HeaderName};

use crate::constants::BEARER_PREFIX;
use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};

/// Header carrying the id of the last Server-Sent Event a client received.
const LAST_EVENT_ID_HEADER: &str = "last-event-id";

/// A bearer token extracted from the `Authorization` header.
///
/// Resolving this extractor requires a well-formed `Authorization: Bearer
/// <token>` header; otherwise the request is rejected with `401 Unauthorized`.
/// This extractor only parses the header. Verifying the token (signature,
/// expiry, claims) is layered on top by application code, typically by a
/// `#[tork::dependency]` that takes a `BearerToken`.
pub struct BearerToken(String);

impl BearerToken {
    /// Returns the raw token, the part of the header after the `Bearer ` prefix.
    pub fn token(&self) -> &str {
        &self.0
    }
}

impl FromRequest for BearerToken {
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let resolved = resolve(ctx);
        async move { resolved }
    }
}

/// The `Last-Event-ID` header sent by a client resuming an SSE stream.
///
/// Resolving this extractor never fails: a missing header yields `None`. Use it
/// to resume a stream from where the client left off.
pub struct LastEventId(Option<String>);

impl LastEventId {
    /// Returns the last event id, if the client sent one.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Consumes the extractor, returning the last event id if present.
    pub fn into_inner(self) -> Option<String> {
        self.0
    }
}

impl FromRequest for LastEventId {
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let id = ctx
            .headers()
            .get(HeaderName::from_static(LAST_EVENT_ID_HEADER))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        async move { Ok(LastEventId(id)) }
    }
}

/// The `Last-Event-ID` header parsed into a typed resume cursor.
///
/// A thin, typed layer over [`LastEventId`] for resuming an SSE stream: the
/// header value is parsed into `T` (a parse failure yields `None`, as does a
/// missing header). Resolving never fails.
pub struct SseResume<T>(Option<T>);

impl<T> SseResume<T> {
    /// Returns the parsed last event id, if the client sent a valid one.
    pub fn last_id(&self) -> Option<&T> {
        self.0.as_ref()
    }

    /// Consumes the extractor, returning the parsed last event id if present.
    pub fn into_inner(self) -> Option<T> {
        self.0
    }
}

impl<T> FromRequest for SseResume<T>
where
    T: std::str::FromStr + Send,
{
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let parsed = ctx
            .headers()
            .get(HeaderName::from_static(LAST_EVENT_ID_HEADER))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<T>().ok());
        async move { Ok(SseResume(parsed)) }
    }
}

/// Parses the bearer token out of the request's `Authorization` header.
fn resolve(ctx: &RequestContext) -> Result<BearerToken> {
    let header = ctx
        .headers()
        .get(AUTHORIZATION)
        .ok_or_else(|| Error::unauthorized("missing Authorization header"))?;

    let value = header
        .to_str()
        .map_err(|_| Error::unauthorized("invalid Authorization header"))?;

    let token = value
        .strip_prefix(BEARER_PREFIX)
        .ok_or_else(|| Error::unauthorized("expected a bearer token"))?;

    Ok(BearerToken(token.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::extract::PathParams;
    use crate::state::StateMap;
    use bytes::Bytes;
    use http_body_util::Full;
    use std::sync::Arc;

    fn context_with(header: Option<(&'static str, &'static str)>) -> RequestContext {
        let mut builder = http::Request::builder();
        if let Some((name, value)) = header {
            builder = builder.header(name, value);
        }
        let head = builder.body(()).unwrap().into_parts().0;
        let body = box_body(Full::new(Bytes::new()));
        RequestContext::new(head, PathParams::new(), Arc::new(StateMap::new()), body)
    }

    #[tokio::test]
    async fn last_event_id_reads_the_header() {
        let ctx = context_with(Some(("last-event-id", "42")));
        let id = LastEventId::from_request(&ctx).await.unwrap();
        assert_eq!(id.as_str(), Some("42"));
    }

    #[tokio::test]
    async fn last_event_id_is_none_when_absent() {
        let ctx = context_with(None);
        let id = LastEventId::from_request(&ctx).await.unwrap();
        assert_eq!(id.into_inner(), None);
    }

    #[tokio::test]
    async fn sse_resume_parses_a_typed_cursor() {
        let ctx = context_with(Some(("last-event-id", "42")));
        let resume = SseResume::<i64>::from_request(&ctx).await.unwrap();
        assert_eq!(resume.last_id().copied(), Some(42));

        // A non-numeric value yields None for an i64 cursor.
        let ctx = context_with(Some(("last-event-id", "abc")));
        let resume = SseResume::<i64>::from_request(&ctx).await.unwrap();
        assert_eq!(resume.into_inner(), None);
    }

    #[tokio::test]
    async fn bearer_token_happy_path() {
        let ctx = context_with(Some(("Authorization", "Bearer abc123")));
        let token = BearerToken::from_request(&ctx).await.unwrap();
        assert_eq!(token.token(), "abc123");
    }

    #[tokio::test]
    async fn bearer_token_missing_header_is_unauthorized() {
        let ctx = context_with(None);
        let error = match BearerToken::from_request(&ctx).await {
            Ok(_) => panic!("missing header must fail"),
            Err(e) => e,
        };
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(error.message(), "missing Authorization header");
    }

    #[tokio::test]
    async fn bearer_token_invalid_utf8_header_is_unauthorized() {
        let mut builder = http::Request::builder();
        builder = builder.header("Authorization", http::HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap());
        let head = builder.body(()).unwrap().into_parts().0;
        let body = box_body(Full::new(Bytes::new()));
        let ctx = RequestContext::new(head, PathParams::new(), Arc::new(StateMap::new()), body);

        let error = match BearerToken::from_request(&ctx).await {
            Ok(_) => panic!("non-utf8 must fail"),
            Err(e) => e,
        };
        assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
        assert_eq!(error.message(), "invalid Authorization header");
    }

    #[tokio::test]
    async fn bearer_token_wrong_scheme_is_unauthorized() {
        for scheme in ["Basic dXNlcjpwYXNz", "Token xyz", "BearerLower xyz", ""] {
            let ctx = context_with(Some(("Authorization", scheme)));
            let error = match BearerToken::from_request(&ctx).await {
                Ok(_) => panic!("scheme `{scheme}` must fail"),
                Err(e) => e,
            };
            assert_eq!(error.kind(), crate::error::ErrorKind::Unauthorized);
            assert_eq!(error.message(), "expected a bearer token");
        }
    }

    #[tokio::test]
    async fn last_event_id_into_inner_some_branch() {
        let ctx = context_with(Some(("last-event-id", "hello")));
        let id = LastEventId::from_request(&ctx).await.unwrap();
        assert_eq!(id.into_inner(), Some("hello".to_owned()));
    }

    #[tokio::test]
    async fn sse_resume_missing_header_yields_none() {
        let ctx = context_with(None);
        let resume = SseResume::<u32>::from_request(&ctx).await.unwrap();
        assert_eq!(resume.last_id(), None);
        assert_eq!(resume.into_inner(), None);
    }

    #[tokio::test]
    async fn sse_resume_valid_value_is_accessible_via_both_accessors() {
        let ctx = context_with(Some(("last-event-id", "42")));
        let resume = SseResume::<u32>::from_request(&ctx).await.unwrap();
        assert_eq!(resume.last_id().copied(), Some(42));
        assert_eq!(resume.into_inner(), Some(42));
    }
}
