//! The statement AST built by the query builder.
//!
//! A [`SelectStatement`] is the backend-neutral description of a query. The
//! builder ([`QuerySet`](crate::query::QuerySet)) assembles it, and a
//! [`Dialect`](crate::dialect::Dialect) renders it to SQL plus bound parameters.

use crate::query::expr::Expr;

/// One item in a `SELECT` projection.
///
/// This phase selects plain columns; aggregate and aliased expressions are added
/// when projection support lands.
#[derive(Debug, Clone)]
pub enum SelectItem {
    /// A qualified column, `"table"."column"`.
    Column {
        /// The owning table.
        table: &'static str,
        /// The column name.
        column: &'static str,
    },
}

/// A single `ORDER BY` term.
#[derive(Debug, Clone)]
pub struct OrderItem {
    /// The expression to order by.
    pub expr: Expr,
    /// Whether to sort descending.
    pub descending: bool,
}

impl OrderItem {
    /// Builds an order term.
    pub fn new(expr: Expr, descending: bool) -> Self {
        Self { expr, descending }
    }
}

/// A `SELECT` statement.
#[derive(Debug, Clone)]
pub struct SelectStatement {
    /// The table being queried.
    pub table: &'static str,
    /// The projected items.
    pub projection: Vec<SelectItem>,
    /// The top-level predicates, joined by `AND`.
    pub filters: Vec<Expr>,
    /// The ordering terms.
    pub order_by: Vec<OrderItem>,
    /// An optional row limit.
    pub limit: Option<u64>,
    /// An optional row offset.
    pub offset: Option<u64>,
    /// Whether to return distinct rows.
    pub distinct: bool,
}

impl SelectStatement {
    /// Builds a statement selecting the given columns from `table`.
    pub fn new(table: &'static str, projection: Vec<SelectItem>) -> Self {
        Self {
            table,
            projection,
            filters: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
            distinct: false,
        }
    }
}
