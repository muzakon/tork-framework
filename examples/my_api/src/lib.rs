//! Example application built on the Tork web framework.
//!
//! The crate is organized the way a real Tork service would be: application
//! state and auth under `core`, serializable models under `models`, data access
//! under `repositories`, business logic under `services`, and routers under
//! `routers`. The binary in `main.rs` wires them together.

pub mod core;
pub mod models;
pub mod repositories;
pub mod routers;
pub mod services;
