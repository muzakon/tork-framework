//! Type-erased application state and the [`State`] extractor.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use crate::error::{Error, Result};
use crate::extract::{FromRequest, RequestContext};

/// A type-erased, thread-safe container for application state values.
///
/// Each value is stored under its [`TypeId`], so a state value is retrieved by
/// its type. This lets routers and handlers stay free of any state type
/// parameter: the [`App`](crate::App) is not generic over its state, which is
/// what allows router modules to be built without knowing the concrete state
/// type.
#[derive(Default)]
pub struct StateMap {
    entries: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl StateMap {
    /// Creates an empty state map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a state value, replacing any existing value of the same type.
    pub fn insert<S: Send + Sync + 'static>(&mut self, value: S) {
        if self.entries.contains_key(&TypeId::of::<S>()) {
            warn!(
                target: "tork",
                "state value of type `{}` is being silently replaced",
                std::any::type_name::<S>(),
            );
        }
        self.entries.insert(TypeId::of::<S>(), Arc::new(value));
    }

    /// Returns a shared handle to the stored value of type `S`, if present.
    pub fn get<S: Send + Sync + 'static>(&self) -> Option<Arc<S>> {
        self.entries
            .get(&TypeId::of::<S>())
            .and_then(|entry| entry.clone().downcast::<S>().ok())
    }

    /// Returns `true` if a value of type `S` is stored.
    pub fn contains<S: Send + Sync + 'static>(&self) -> bool {
        self.entries.contains_key(&TypeId::of::<S>())
    }

    /// Removes the stored value of type `S`, if present.
    pub fn remove<S: Send + Sync + 'static>(&mut self) {
        self.entries.remove(&TypeId::of::<S>());
    }
}

/// A shared, reference-counted handle to the application state map.
pub type AppStateRef = Arc<StateMap>;

/// Extractor that yields a clone of an application state value of type `S`.
///
/// The wrapped value is cloned out of the shared state on each request, so `S`
/// should be cheap to clone (for example, hold connection pools or other handles
/// behind `Arc`).
///
/// # Errors
///
/// Resolving fails with an internal error if no value of type `S` was registered
/// with [`App::state`](crate::App::state).
pub struct State<S>(pub S);

impl<S> FromRequest for State<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn from_request(
        ctx: &RequestContext,
    ) -> impl std::future::Future<Output = Result<Self>> + Send {
        let resolved = match ctx.state().get::<S>() {
            Some(value) => Ok(State((*value).clone())),
            None => Err(Error::internal(format!(
                "application state `{}` was not configured",
                std::any::type_name::<S>()
            ))),
        };
        async move { resolved }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Config {
        name: String,
    }

    #[test]
    fn insert_and_get_by_type() {
        let mut map = StateMap::new();
        map.insert(Config {
            name: "tork".to_owned(),
        });

        let config = map.get::<Config>().expect("config should be present");
        assert_eq!(config.name, "tork");
        assert!(map.get::<u32>().is_none());
        assert!(map.contains::<Config>());
    }
}
