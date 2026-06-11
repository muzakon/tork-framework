//! Procedural macros for the Tork ORM.
//!
//! Every macro here emits code that refers to the ORM's public API through the
//! `tork-orm` facade crate (for example `::tork_orm::Value`), never through
//! `tork-orm-core` directly, so generated code compiles inside user crates that
//! depend only on `tork-orm`.

use proc_macro::TokenStream;

mod common;
mod model;
mod relations;

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

/// Declares the relations of a model on an `impl` block.
///
/// Each method names a relation and is rewritten into an accessor returning a
/// [`Relation`] descriptor used by `QuerySet::join` (and, later, preloading).
///
/// # Method attributes
///
/// - `#[has_many(Other, foreign_key = Other::this_id)]` — a one-to-many where the
///   other model carries this model's key
/// - `#[belongs_to(Other, foreign_key = Self::other_id)]` — a many-to-one where
///   this model carries the other model's key
///
/// # Example
///
/// ```ignore
/// #[relations]
/// impl User {
///     #[has_many(Post, foreign_key = Post::user_id)]
///     pub fn posts() {}
/// }
/// // `User::posts()` now returns a `Relation<User, Post>`.
/// ```
#[proc_macro_attribute]
pub fn relations(_attr: TokenStream, item: TokenStream) -> TokenStream {
    relations::expand(item)
}
