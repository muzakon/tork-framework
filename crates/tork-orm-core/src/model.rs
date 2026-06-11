//! The [`Model`] trait and the metadata a `#[derive(Model)]` produces.
//!
//! A model is a Rust struct that mirrors a database table. The derive generates
//! the table name, a description of every column, and the conversions between a
//! row and an instance. The column metadata is intentionally richer than query
//! execution needs today (it records SQL types and foreign keys) so that a later
//! migrations phase can build on it.

use crate::dialect::SqlType;
use crate::query::QuerySet;
use crate::row::Row;
use crate::value::Value;

/// A foreign key reference recorded on a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignKeyDef {
    /// The referenced table.
    pub table: &'static str,
    /// The referenced column in that table.
    pub column: &'static str,
}

/// The compile-time description of a single model column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnDef {
    /// The column name in the database.
    pub name: &'static str,
    /// The abstract SQL type of the column.
    pub sql_type: SqlType,
    /// Whether the column is (part of) the primary key.
    pub primary_key: bool,
    /// Whether the database assigns the value automatically (auto-increment).
    pub auto: bool,
    /// Whether the column accepts `NULL` (the Rust field is an `Option`).
    pub nullable: bool,
    /// A foreign key reference, if the column points at another table.
    pub foreign_key: Option<ForeignKeyDef>,
}

/// Builds an instance from a result row.
///
/// Implemented by `#[derive(Model)]` for full models and by
/// `#[derive(QueryResult)]` for projection DTOs. Mapping is by column name, so the
/// order of selected columns does not have to match the field order.
pub trait FromRow: Sized {
    /// Reads each field from its like-named column in `row`.
    fn from_row(row: &Row) -> crate::Result<Self>;
}

/// A struct that maps to a database table.
///
/// # Examples
///
/// ```
/// use tork_orm_core::{ColumnDef, Model};
///
/// fn primary_key<M: Model>() -> &'static str {
///     M::PRIMARY_KEY
/// }
/// ```
pub trait Model: FromRow + Send + Sync + 'static {
    /// The table this model maps to.
    const TABLE: &'static str;
    /// The description of every column, in declaration order.
    const COLUMNS: &'static [ColumnDef];
    /// The name of the primary key column.
    const PRIMARY_KEY: &'static str;

    /// Returns the column-name and value pairs to write on insert.
    ///
    /// Auto-assigned columns (such as an auto-increment primary key) are omitted
    /// so the database fills them in.
    fn insert_values(&self) -> Vec<(&'static str, Value)>;

    /// Returns the value of the primary key column for this instance.
    fn primary_key_value(&self) -> Value;

    /// Starts a query over this model.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tork_orm_core::{Database, Model};
    /// # async fn run<M: Model>(db: Database) -> tork_orm_core::Result<()> {
    /// let rows = M::query().limit(10).all(&db).await?;
    /// # let _ = rows;
    /// # Ok(())
    /// # }
    /// ```
    fn query() -> QuerySet<Self>
    where
        Self: Sized,
    {
        QuerySet::new()
    }
}
