//! Path-parameter parsing.

use std::fmt::Display;
use std::str::FromStr;

use crate::error::{Error, Result};
use crate::extract::RequestContext;

/// Parses a single captured URL segment into a typed path parameter.
///
/// A blanket implementation covers every type that implements [`FromStr`], so
/// `i64`, `String`, `Uuid`, and similar types work out of the box.
pub trait FromPathParam: Sized {
    /// Parses `value`, the raw URL segment captured for the parameter `name`.
    ///
    /// `name` is used only for diagnostics; the raw value is never echoed back in
    /// error messages.
    fn from_path_param(name: &str, value: &str) -> Result<Self>;
}

impl<T> FromPathParam for T
where
    T: FromStr,
    T::Err: Display,
{
    fn from_path_param(name: &str, value: &str) -> Result<Self> {
        value.parse::<T>().map_err(|_| {
            Error::unprocessable(format!("invalid value for path parameter `{name}`"))
        })
    }
}

/// Resolves and parses the path parameter named `name` from the request.
///
/// This is generated-code support, not part of the user-facing API. A missing
/// parameter indicates a routing bug (the route matched but the placeholder was
/// absent), so it is reported as an internal error rather than a client error.
#[doc(hidden)]
pub fn __extract_path_param<T: FromPathParam>(ctx: &RequestContext, name: &str) -> Result<T> {
    match ctx.path_param(name) {
        Some(value) => T::from_path_param(name, value),
        None => Err(Error::internal(format!(
            "path parameter `{name}` was not captured by the router"
        ))),
    }
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

    fn context(params: PathParams) -> RequestContext {
        let head = http::Request::new(()).into_parts().0;
        let body = box_body(Full::new(Bytes::new()));
        RequestContext::new(head, params, Arc::new(StateMap::new()), body)
    }

    #[test]
    fn missing_param_is_internal_router_error() {
        let error = __extract_path_param::<i64>(&context(PathParams::new()), "user_id").unwrap_err();

        assert_eq!(error.kind(), crate::error::ErrorKind::Internal);
        assert_eq!(
            error.message(),
            "path parameter `user_id` was not captured by the router"
        );
    }

    #[test]
    fn invalid_param_is_unprocessable() {
        let mut params = PathParams::new();
        params.push("user_id".to_owned(), "abc".to_owned());

        let error = __extract_path_param::<i64>(&context(params), "user_id").unwrap_err();
        assert_eq!(error.kind(), crate::error::ErrorKind::Unprocessable);
        assert_eq!(error.message(), "invalid value for path parameter `user_id`");
    }
}
