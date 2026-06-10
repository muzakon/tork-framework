//! Validating request-body extractor.

use garde::Validate;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::extract::body::read_body_capped;
use crate::extract::{FromRequest, RequestContext};

/// Deserializes the JSON request body into `T` and validates it.
///
/// `T` is usually an `#[api_model]` type. Deserialization failures produce a
/// `422 Unprocessable Entity`, and validation failures produce a `422` whose
/// body lists the offending fields (see [`Error::from_garde_report`]).
///
/// Access the inner value via the `.0` field or [`Valid::into_inner`].
#[derive(Debug, Clone)]
pub struct Valid<T>(pub T);

impl<T> Valid<T> {
    /// Unwraps the validated value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> FromRequest for Valid<T>
where
    T: DeserializeOwned + Validate<Context = ()> + Send,
{
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let taken = ctx.take_body();
        async move {
            let bytes = read_body_capped(taken?).await?;
            let value: T = serde_json::from_slice(&bytes)
                .map_err(|_| Error::unprocessable("request body is not valid JSON"))?;
            value.validate().map_err(Error::from_garde_report)?;
            Ok(Valid(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::error::ErrorKind;
    use crate::extract::PathParams;
    use crate::state::StateMap;

    use bytes::Bytes;
    use http_body_util::Full;
    use serde::Deserialize;
    use std::sync::Arc;

    #[derive(Debug, Deserialize, garde::Validate)]
    struct Sample {
        #[garde(range(min = 1))]
        count: i64,
    }

    fn context_with_body(json: &'static str) -> RequestContext {
        let head = http::Request::new(()).into_parts().0;
        let body = box_body(Full::new(Bytes::from_static(json.as_bytes())));
        RequestContext::new(head, PathParams::new(), Arc::new(StateMap::new()), body)
    }

    #[tokio::test]
    async fn valid_body_is_accepted() {
        let ctx = context_with_body(r#"{"count": 5}"#);
        let valid = <Valid<Sample> as FromRequest>::from_request(&ctx)
            .await
            .expect("should validate");
        assert_eq!(valid.into_inner().count, 5);
    }

    #[tokio::test]
    async fn invalid_body_is_unprocessable_with_details() {
        let ctx = context_with_body(r#"{"count": 0}"#);
        let error = <Valid<Sample> as FromRequest>::from_request(&ctx)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unprocessable);
        assert!(!error.details().is_empty(), "should report the failing field");
    }
}
