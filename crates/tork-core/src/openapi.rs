//! Hook for OpenAPI document generation.

use crate::router::Route;

/// Produces the routes that serve the OpenAPI document and documentation UI.
///
/// This trait is implemented by the OpenAPI crate and registered through
/// [`App::openapi`](crate::App::openapi). Defining the hook here, instead of
/// depending on the OpenAPI crate, keeps the dependency graph acyclic and lets
/// OpenAPI support be compiled out entirely.
pub trait OpenApiProvider: Send + Sync + 'static {
    /// Given the application's registered routes, returns extra routes that serve
    /// the specification document and the documentation UI.
    fn documentation_routes(&self, registered: &[Route]) -> Vec<Route>;
}

/// Produces the routes that serve the AsyncAPI document.
///
/// The mirror of [`OpenApiProvider`] for the event-driven side: it describes the
/// Server-Sent Events and WebSocket channels. Implemented by the OpenAPI crate
/// and registered through [`App::asyncapi`](crate::App::asyncapi).
pub trait AsyncApiProvider: Send + Sync + 'static {
    /// Given the application's registered routes, returns extra routes that serve
    /// the AsyncAPI document.
    fn documentation_routes(&self, registered: &[Route]) -> Vec<Route>;
}
