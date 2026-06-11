//! The SQLite dialect.

use super::{Dialect, SqlType};

/// SQL generation for SQLite.
///
/// SQLite quotes identifiers with double quotes, uses positional `?`
/// placeholders, and supports `RETURNING`. Its dynamic typing means the abstract
/// types map onto a small set of storage classes.
///
/// # Examples
///
/// ```
/// use tork_orm_core::dialect::{Dialect, SqliteDialect};
///
/// let dialect = SqliteDialect::new();
/// assert_eq!(dialect.quoted("user id"), "\"user id\"");
/// let mut placeholder = String::new();
/// dialect.placeholder(0, &mut placeholder);
/// assert_eq!(placeholder, "?");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl SqliteDialect {
    /// Creates a SQLite dialect.
    pub const fn new() -> Self {
        Self
    }
}

impl Dialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn quote_identifier(&self, identifier: &str, out: &mut String) {
        out.push('"');
        for ch in identifier.chars() {
            if ch == '"' {
                // Double an embedded quote so it cannot terminate the identifier.
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    }

    fn placeholder(&self, _index: usize, out: &mut String) {
        out.push('?');
    }

    fn supports_returning(&self) -> bool {
        true
    }

    fn map_sql_type(&self, ty: SqlType) -> &'static str {
        match ty {
            SqlType::Boolean | SqlType::Integer | SqlType::BigInt => "INTEGER",
            SqlType::Real => "REAL",
            SqlType::Text | SqlType::Varchar(_) | SqlType::Timestamp => "TEXT",
            SqlType::Blob => "BLOB",
        }
    }
}
