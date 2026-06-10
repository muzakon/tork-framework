//! The application builder and its finalized, request-handling core.

use std::sync::Arc;

use crate::error::Result;
use crate::openapi::OpenApiProvider;
use crate::router::Router;
use crate::router::matcher::Matcher;
use crate::state::{AppStateRef, StateMap};

/// The application builder.
///
/// `App` collects application state, routers, and optional OpenAPI configuration,
/// then either finalizes into an [`AppInner`] via [`App::build`] or starts
/// serving via [`App::serve`](crate::App::serve).
///
/// `App` is deliberately not generic over its state type: state is stored in a
/// type-erased [`StateMap`], which is what lets router modules be defined without
/// any knowledge of the concrete state type.
pub struct App {
    state: StateMap,
    routers: Vec<Router>,
    openapi: Option<Box<dyn OpenApiProvider>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Creates an empty application.
    pub fn new() -> Self {
        Self {
            state: StateMap::new(),
            routers: Vec::new(),
            openapi: None,
        }
    }

    /// Registers an application state value, retrievable via the
    /// [`State`](crate::State) extractor.
    pub fn state<S: Send + Sync + 'static>(mut self, state: S) -> Self {
        self.state.insert(state);
        self
    }

    /// Mounts a router's routes on the application.
    pub fn include_router(mut self, router: Router) -> Self {
        self.routers.push(router);
        self
    }

    /// Configures OpenAPI document generation and the documentation UI.
    pub fn openapi<P: OpenApiProvider>(mut self, provider: P) -> Self {
        self.openapi = Some(Box::new(provider));
        self
    }

    /// Finalizes the application into its request-handling core.
    ///
    /// Routers are flattened into a single route table, OpenAPI documentation
    /// routes (if configured) are appended, and a [`Matcher`] is compiled.
    ///
    /// # Errors
    ///
    /// Returns an error if the route table contains an invalid or duplicate path.
    pub fn build(self) -> Result<AppInner> {
        let App {
            state,
            routers,
            openapi,
        } = self;

        let mut routes = Vec::new();
        for router in routers {
            routes.extend(router.into_routes());
        }

        if let Some(provider) = openapi {
            let documentation = provider.documentation_routes(&routes);
            routes.extend(documentation);
        }

        let matcher = Matcher::build(routes)?;

        Ok(AppInner {
            state: Arc::new(state),
            matcher,
        })
    }
}

/// The finalized application: shared state plus a compiled route matcher.
///
/// This is the value shared across all connections. It is cheap to clone behind
/// an `Arc` and is what the server hands each request to via
/// [`dispatch`](AppInner::dispatch).
pub struct AppInner {
    state: AppStateRef,
    matcher: Matcher,
}

impl AppInner {
    /// Returns the shared application state.
    pub fn state(&self) -> &AppStateRef {
        &self.state
    }

    /// Returns the compiled route matcher.
    pub fn matcher(&self) -> &Matcher {
        &self.matcher
    }
}
