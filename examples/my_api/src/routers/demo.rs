//! Routes that demonstrate the hooks and error-handling surface.

use tork::{api_router, get};

use crate::core::errors::RepoError;

#[api_router(prefix = "/demo", tags = ["demo"])]
pub mod demo_router {
    use super::*;

    /// Surfaces a typed `RepoError`, mapped by the registered exception handler.
    #[get("/db-error", summary = "Trigger a typed error")]
    pub async fn db_error() -> tork::Result<i64> {
        // The `?` converts `RepoError` into `tork::Error`, carrying the typed source.
        let outcome: Result<i64, RepoError> = Err(RepoError::Unavailable);
        Ok(outcome?)
    }

    /// Panics on purpose, to demonstrate the panic boundary.
    #[get("/panic", summary = "Trigger a handler panic")]
    pub async fn boom() -> tork::Result<i64> {
        panic!("demo panic");
    }
}

/// Builds the demo router.
pub fn router() -> tork::Router {
    demo_router::router()
}
