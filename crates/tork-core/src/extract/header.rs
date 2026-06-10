//! Header-based extractors.

use http::header::AUTHORIZATION;

use crate::constants::BEARER_PREFIX;
use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};

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
