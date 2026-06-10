//! Example application built on the Tork web framework.
//!
//! The crate is organized the way a real Tork service would be: application
//! state under `core`, serializable models under `models`, and routers under
//! `routers`. The binary in `main.rs` wires them together.

pub mod core;
pub mod models;
pub mod routers;
