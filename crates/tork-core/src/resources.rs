//! The resource registry contract.
//!
//! A resource container declares which of its fields are resources. At startup
//! each resource is inserted into the application registry by type, so services
//! and handlers can inject it directly rather than reaching through a state
//! object.

use crate::state::StateMap;

/// A type that contributes resources to the application registry.
///
/// Implemented by `#[derive(Resources)]`, which registers each `#[resource]`
/// field (by a clone) under its type.
pub trait Resources: Send + Sync + 'static {
    /// Inserts each declared resource into `registry`.
    fn register(&self, registry: &mut StateMap);
}
