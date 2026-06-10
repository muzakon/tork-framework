//! Procedural macros for the Tork web framework.
//!
//! Every macro here emits code that refers to the public API through the `tork`
//! facade crate (for example `::tork::Router`), never through `tork-core`
//! directly. This lets generated code compile inside user crates that depend
//! only on `tork`.
