//! Procedural macros for the Tork ORM.
//!
//! Every macro here emits code that refers to the ORM's public API through the
//! `tork-orm` facade crate (for example `::tork_orm::Value`), never through
//! `tork-orm-core` directly, so generated code compiles inside user crates that
//! depend only on `tork-orm`.

use proc_macro::TokenStream;

mod common;
mod model;

/// Derives the [`Model`] trait for a struct that maps to a database table.
///
/// Generates the table metadata, a `FromRow` implementation, and the insert and
/// primary-key value accessors.
///
/// # Container attribute
///
/// - `#[table(name = "users")]` sets the table name (defaults to the struct name
///   in `snake_case`).
///
/// # Field attributes (`#[field(...)]`)
///
/// - `primary_key` marks the primary key column (exactly one is required)
/// - `auto` marks a database-assigned value, omitted on insert
/// - `varchar(length = N)` records a bounded text type
/// - `foreign_key = Other::column` records a foreign key reference
/// - `column = "name"` overrides the column name (defaults to the field name)
///
/// # Example
///
/// ```ignore
/// #[derive(Debug, Clone, Model)]
/// #[table(name = "users")]
/// pub struct User {
///     #[field(primary_key, auto)]
///     pub id: i64,
///     #[field(varchar(length = 50))]
///     pub username: String,
///     pub is_active: bool,
/// }
/// ```
#[proc_macro_derive(Model, attributes(table, field))]
pub fn derive_model(item: TokenStream) -> TokenStream {
    model::expand(item)
}
