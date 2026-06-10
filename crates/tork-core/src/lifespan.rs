//! Application lifespan: typed startup and shutdown tied to a resource container.

use std::future::Future;
use std::net::SocketAddr;

use crate::error::{Error, Result};
use crate::resources::Resources;

/// The startup and shutdown lifecycle of a resource container.
///
/// `startup` builds the container (acquiring pools, loading config, spawning
/// workers); the value it returns has its resources registered for injection.
/// `shutdown` releases those resources and is optional (it defaults to a no-op).
///
/// Implemented by `#[tork::lifespan]`. A lifespan type must also be a
/// [`Resources`] container.
pub trait Lifespan: Resources + Sized {
    /// Builds the resource container.
    fn startup(ctx: LifespanContext) -> impl Future<Output = Result<Self>> + Send;

    /// Releases the container's resources. Defaults to a no-op.
    fn shutdown(self) -> impl Future<Output = Result<()>> + Send {
        async move {
            let _ = self;
            Ok(())
        }
    }
}

/// Context passed to a lifespan's `startup`.
///
/// Provides access to the process environment and is constructed by the
/// framework when the application boots.
pub struct LifespanContext {
    _private: (),
}

impl LifespanContext {
    /// Creates a lifespan context.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Reads a required environment variable.
    ///
    /// # Errors
    ///
    /// Returns an error (code `MISSING_ENV`) if the variable is not set.
    pub fn env(&self, key: &str) -> Result<String> {
        std::env::var(key).map_err(|_| {
            Error::internal(format!("required environment variable `{key}` is not set"))
                .with_code("MISSING_ENV")
        })
    }
}

impl Default for LifespanContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Context passed to `on_ready` hooks, after the listener has bound.
#[derive(Clone, Debug)]
pub struct ReadyContext {
    addr: SocketAddr,
}

impl ReadyContext {
    /// Creates a ready context for a bound address.
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    /// Returns the bound local address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_env_is_an_error() {
        let ctx = LifespanContext::new();
        let error = ctx.env("TORK_DEFINITELY_MISSING_VARIABLE_XYZ").unwrap_err();
        assert_eq!(error.code(), "MISSING_ENV");
    }
}
