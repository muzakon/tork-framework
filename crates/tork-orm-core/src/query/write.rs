//! The write-statement AST: inserts, updates, and deletes.
//!
//! Like the [`SelectStatement`](crate::query::ast::SelectStatement), these are
//! backend-neutral and rendered to SQL plus bound parameters by a dialect. All
//! written values are bound parameters.

use crate::query::expr::Expr;
use crate::value::Value;

/// A column assignment in an `UPDATE` (`column = value`).
///
/// Built by [`Column::set`](crate::Column::set), so the assigned value is checked
/// against the column's type.
#[derive(Debug, Clone)]
pub struct Assignment {
    /// The column being assigned.
    pub column: &'static str,
    /// The bound value to assign.
    pub value: Value,
}

impl Assignment {
    /// Builds an assignment of `value` to `column`.
    pub fn new(column: &'static str, value: Value) -> Self {
        Self { column, value }
    }
}

/// An `INSERT` statement, possibly inserting several rows.
#[derive(Debug, Clone)]
pub struct InsertStatement {
    /// The target table.
    pub table: &'static str,
    /// The columns written, in order.
    pub columns: Vec<&'static str>,
    /// One value list per inserted row, each aligned with `columns`.
    pub rows: Vec<Vec<Value>>,
    /// Columns to return from the inserted rows; empty means no `RETURNING`.
    pub returning: Vec<&'static str>,
}

/// An `UPDATE` statement.
#[derive(Debug, Clone)]
pub struct UpdateStatement {
    /// The target table.
    pub table: &'static str,
    /// The column assignments.
    pub assignments: Vec<Assignment>,
    /// The predicates restricting which rows change, joined by `AND`.
    pub filters: Vec<Expr>,
}

/// A `DELETE` statement.
#[derive(Debug, Clone)]
pub struct DeleteStatement {
    /// The target table.
    pub table: &'static str,
    /// The predicates restricting which rows are removed, joined by `AND`.
    pub filters: Vec<Expr>,
}
