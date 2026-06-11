//! Procedural macros for the Tork ORM.
//!
//! Every macro here emits code that refers to the ORM's public API through the
//! `tork-orm` facade crate (for example `::tork_orm::Value`), never through
//! `tork-orm-core` directly, so generated code compiles inside user crates that
//! depend only on `tork-orm`.
//!
//! The derives and attributes (`Model`, `QueryResult`, `relations`) are added in
//! the following commits of this phase.
