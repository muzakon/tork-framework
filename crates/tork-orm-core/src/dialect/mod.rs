//! The dialect abstraction: everything backend-specific about generating SQL.
//!
//! A [`Dialect`] knows how to quote identifiers, write parameter placeholders, map
//! abstract column types to concrete SQL types, and (in later phases) render the
//! query AST into a SQL string plus an ordered list of bound parameters. Keeping
//! these behind one trait is what makes adding a new backend a small, isolated
//! change: implement [`Dialect`] for it and wire up a driver.

pub mod writer;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteDialect;

pub use writer::{
    QueryWriter, render_count, render_delete, render_exists, render_expr, render_insert,
    render_select, render_update,
};

/// An abstract column type, independent of any backend.
///
/// Models record one of these per column (derived from the field's Rust type and
/// `#[field(...)]` attributes). A dialect maps it to a concrete SQL type. The
/// mapping is unused by query execution today but is the foundation a later
/// migrations phase builds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlType {
    /// A boolean, typically stored as a small integer.
    Boolean,
    /// A 32-bit signed integer.
    Integer,
    /// A 64-bit signed integer.
    BigInt,
    /// A double-precision floating point number.
    Real,
    /// Unbounded UTF-8 text.
    Text,
    /// Bounded UTF-8 text of at most the given length.
    Varchar(u32),
    /// A timestamp with time zone.
    Timestamp,
    /// Raw bytes.
    Blob,
}

/// Generates backend-specific SQL.
///
/// Implementors override the small set of primitives that differ between
/// databases. The query layer (added in a later commit of this phase) renders the
/// query AST through these primitives, so the AST itself stays backend-neutral.
pub trait Dialect: Send + Sync + 'static {
    /// Returns the dialect's stable name, for diagnostics.
    fn name(&self) -> &'static str;

    /// Writes a quoted identifier (table or column name) into `out`.
    ///
    /// Implementations must escape the quote character to prevent identifiers
    /// from breaking out of their quoting.
    fn quote_identifier(&self, identifier: &str, out: &mut String);

    /// Writes the placeholder for the parameter at `index` (zero-based) into `out`.
    ///
    /// Backends differ here: some use a positional `?`, others a numbered `$N`.
    fn placeholder(&self, index: usize, out: &mut String);

    /// Returns `true` if the backend supports `INSERT ... RETURNING`.
    fn supports_returning(&self) -> bool;

    /// Maps an abstract [`SqlType`] to the backend's concrete type keyword.
    fn map_sql_type(&self, ty: SqlType) -> &'static str;

    /// Returns a quoted identifier as an owned `String`.
    ///
    /// A convenience wrapper over [`Dialect::quote_identifier`].
    fn quoted(&self, identifier: &str) -> String {
        let mut out = String::with_capacity(identifier.len() + 2);
        self.quote_identifier(identifier, &mut out);
        out
    }
}
