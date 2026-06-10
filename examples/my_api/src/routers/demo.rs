//! Routes that demonstrate the hooks, error-handling, and streaming surface.

use tork::{LastEventId, RequestEvent, Sse, SseEvent, api_router, get, sse};

use crate::core::errors::RepoError;
use crate::models::event::EventOut;

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

    /// Streams a few events over Server-Sent Events.
    ///
    /// Honors `Last-Event-ID` so a reconnecting client resumes after the last
    /// sequence number it saw.
    #[sse(
        "/stream",
        event = "tick",
        response_model = EventOut,
        summary = "Stream demo events"
    )]
    pub async fn stream(last: LastEventId) -> tork::Result<Sse<EventOut>> {
        let start = last
            .as_str()
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(0);

        let events = futures_util::stream::iter((start + 1..=start + 3).map(|sequence| {
            let out = EventOut {
                sequence,
                message: format!("event {sequence}"),
            };
            // Set the event id so a reconnecting client can resume from it.
            Ok::<_, tork::Error>(SseEvent::new(out).id(sequence))
        }));

        Ok(Sse::events(events))
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
