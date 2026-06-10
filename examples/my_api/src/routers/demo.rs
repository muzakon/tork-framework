//! Routes that demonstrate the hooks and error-handling surface.

use tork::{RequestEvent, api_router, get};

use crate::core::errors::RepoError;

#[api_router(prefix = "/demo", tags = ["demo"])]
pub mod demo_router {
    use super::*;

    /// Surfaces a typed `RepoError`, mapped by the registered exception handler.
    ///
    /// Carries a route-level `on_request` hook (the named function below), so it
    /// runs in addition to the router-level audit hook.
    #[get("/db-error", on_request = note_db_call, summary = "Trigger a typed error")]
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

/// Router-level audit hook: runs for every route in the demo router.
async fn audit_request(event: RequestEvent) {
    println!("audit: demo request {} {}", event.method(), event.path());
}

/// Route-level hook: runs only for the `/demo/db-error` route.
async fn note_db_call(event: RequestEvent) {
    println!("audit: db-error route touched on {}", event.path());
}

/// Builds the demo router with a router-scoped audit hook.
pub fn router() -> tork::Router {
    demo_router::router().on_request(audit_request)
}
