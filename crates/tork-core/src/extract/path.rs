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
