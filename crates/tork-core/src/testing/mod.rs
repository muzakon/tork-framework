//! In-process test harness.
//!
//! [`TestClient`] drives an application without a network: it builds the app
//! (running the lifespan), then sends requests straight through the request
//! pipeline and reads responses, with helpers for JSON, forms, file uploads,
//! Server-Sent Events, and WebSockets. It can also override resources and
//! dependencies, hold a cookie jar and default headers, and run the lifespan
//! shutdown when finished.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::extract::RequestContext;

mod client;
mod cookie;
mod recorder;
mod request;
mod response;
mod sse;
mod websocket;

pub use client::{TestClient, TestClientBuilder};
pub use recorder::{LogRecord, LogRecorder};
pub use request::{TestMultipartBuilder, TestRequestBuilder};
pub use response::TestResponse;
pub use sse::{TestSseEvent, TestSseStream};
pub use websocket::{TestWebSocket, TestWebSocketBuilder};

/// A factory producing a fresh boxed instance of an overridden dependency.
type OverrideFactory = Arc<dyn Fn() -> Box<dyn Any + Send> + Send + Sync>;

/// Test-time dependency overrides, keyed by the injected type.
///
/// Inserted into the application state by the test client. The `#[derive(Inject)]`
/// implementation consults it (through [`__take_override`]) before building a
/// service from its fields, so a test can substitute a pre-built instance.
#[derive(Default, Clone)]
pub(crate) struct TestOverrides {
    factories: HashMap<TypeId, OverrideFactory>,
}

impl TestOverrides {
    /// Registers a factory that produces the override value for `T`.
    // Called by the test client builder, which lands in a later commit.
    #[allow(dead_code)]
    pub(crate) fn insert<T, F>(&mut self, factory: F)
    where
        T: Send + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.factories
            .insert(TypeId::of::<T>(), Arc::new(move || Box::new(factory())));
    }

    /// Produces a fresh override value for `T`, if one is registered.
    fn produce<T: 'static>(&self) -> Option<T> {
        let factory = self.factories.get(&TypeId::of::<T>())?;
        factory().downcast::<T>().ok().map(|boxed| *boxed)
    }

    /// Returns `true` if no overrides are registered.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

/// Returns a test override for `T`, if the test client registered one.
///
/// Generated-code support for `#[derive(Inject)]`; not part of the public API. In
/// a normal build no overrides are registered, so this is a single state lookup
/// that returns `None`.
#[doc(hidden)]
pub fn __take_override<T: 'static>(ctx: &RequestContext) -> Option<T> {
    ctx.state().get::<TestOverrides>()?.produce::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::box_body;
    use crate::extract::PathParams;
    use crate::state::StateMap;
    use bytes::Bytes;
    use http_body_util::Full;

    fn context_with_overrides(overrides: TestOverrides) -> RequestContext {
        let mut state = StateMap::new();
        state.insert(overrides);
        let head = http::Request::new(()).into_parts().0;
        RequestContext::new(
            head,
            PathParams::new(),
            Arc::new(state),
            box_body(Full::new(Bytes::new())),
        )
    }

    #[test]
    fn override_registry_reports_empty_and_produces_fresh_values() {
        let mut overrides = TestOverrides::default();
        assert!(overrides.is_empty());

        overrides.insert::<String, _>(|| "hello".to_owned());
        assert!(!overrides.is_empty());
        assert_eq!(overrides.produce::<String>().as_deref(), Some("hello"));
    }

    #[test]
    fn take_override_reads_registered_override() {
        let mut overrides = TestOverrides::default();
        overrides.insert::<usize, _>(|| 7usize);

        let ctx = context_with_overrides(overrides);
        assert_eq!(__take_override::<usize>(&ctx), Some(7));
        assert_eq!(__take_override::<String>(&ctx), None);
    }
}
