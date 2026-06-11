//! The shared SQL writer.
//!
//! A [`QueryWriter`] accumulates SQL text and an ordered list of bound parameters,
//! deferring the backend-specific bits (identifier quoting, placeholder spelling)
//! to a [`Dialect`]. The query layer renders the AST through it, so all dialects
//! share one rendering walk and differ only in their primitives.

use crate::dialect::Dialect;
use crate::query::ast::{SelectItem, SelectStatement};
use crate::query::expr::Expr;
use crate::query::write::{DeleteStatement, InsertStatement, UpdateStatement};
use crate::value::Value;

/// Builds a SQL string and its bound parameters for a dialect.
pub struct QueryWriter<'a> {
    dialect: &'a dyn Dialect,
    sql: String,
    params: Vec<Value>,
}

impl<'a> QueryWriter<'a> {
    /// Creates a writer that renders for `dialect`.
    pub fn new(dialect: &'a dyn Dialect) -> Self {
        Self {
            dialect,
            sql: String::new(),
            params: Vec::new(),
        }
    }

    /// Appends raw SQL text.
    pub fn push_sql(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    /// Appends a quoted identifier.
    pub fn push_identifier(&mut self, identifier: &str) {
        self.dialect.quote_identifier(identifier, &mut self.sql);
    }

    /// Appends a `"table"."column"` reference.
    pub fn push_qualified(&mut self, table: &str, column: &str) {
        self.push_identifier(table);
        self.sql.push('.');
        self.push_identifier(column);
    }

    /// Appends a placeholder and records its bound value.
    pub fn push_bind(&mut self, value: Value) {
        let index = self.params.len();
        self.dialect.placeholder(index, &mut self.sql);
        self.params.push(value);
    }

    /// Renders a boolean expression.
    pub fn write_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Column { table, column } => self.push_qualified(table, column),
            Expr::Value(value) => self.push_bind(value.clone()),
            Expr::Binary { left, op, right } => {
                self.write_expr(left);
                self.sql.push(' ');
                self.push_sql(op.as_sql());
                self.sql.push(' ');
                self.write_expr(right);
            }
            Expr::Logical { op, items } => self.write_logical(*op, items),
            Expr::Not(inner) => {
                self.push_sql("NOT (");
                self.write_expr(inner);
                self.sql.push(')');
            }
            Expr::InList { expr, values } => self.write_in_list(expr, values),
            Expr::IsNull { expr, negated } => {
                self.write_expr(expr);
                self.push_sql(if *negated { " IS NOT NULL" } else { " IS NULL" });
            }
            Expr::Aggregate { func, arg } => {
                self.push_sql(func.as_sql());
                self.sql.push('(');
                self.write_expr(arg);
                self.sql.push(')');
            }
            Expr::CountStar => self.push_sql("COUNT(*)"),
            Expr::Alias { expr, alias } => {
                self.write_expr(expr);
                self.push_sql(" AS ");
                self.push_identifier(alias);
            }
        }
    }

    /// Renders an `AND`/`OR` group, parenthesizing when it joins more than one item.
    fn write_logical(&mut self, op: crate::query::expr::LogicalOp, items: &[Expr]) {
        use crate::query::expr::LogicalOp;
        match items {
            // An empty group is the connective's identity: AND of nothing is true,
            // OR of nothing is false.
            [] => self.push_sql(match op {
                LogicalOp::And => "1 = 1",
                LogicalOp::Or => "0 = 1",
            }),
            [single] => self.write_expr(single),
            many => {
                self.sql.push('(');
                for (index, item) in many.iter().enumerate() {
                    if index != 0 {
                        self.sql.push(' ');
                        self.push_sql(op.as_sql());
                        self.sql.push(' ');
                    }
                    self.write_expr(item);
                }
                self.sql.push(')');
            }
        }
    }

    /// Renders a membership test, collapsing an empty list to a false constant.
    fn write_in_list(&mut self, expr: &Expr, values: &[Value]) {
        if values.is_empty() {
            self.push_sql("0 = 1");
            return;
        }
        self.write_expr(expr);
        self.push_sql(" IN (");
        for (index, value) in values.iter().enumerate() {
            if index != 0 {
                self.push_sql(", ");
            }
            self.push_bind(value.clone());
        }
        self.sql.push(')');
    }

    /// Renders the `WHERE` clause for a statement's top-level filters.
    ///
    /// The filters are joined by `AND` without an outer parenthesis; any nested
    /// group renders its own parentheses.
    fn write_where(&mut self, filters: &[Expr]) {
        if filters.is_empty() {
            return;
        }
        self.push_sql(" WHERE ");
        for (index, filter) in filters.iter().enumerate() {
            if index != 0 {
                self.push_sql(" AND ");
            }
            self.write_expr(filter);
        }
    }

    /// Renders a `SELECT` statement.
    pub fn write_select(&mut self, statement: &SelectStatement) {
        self.push_sql("SELECT ");
        if statement.distinct {
            self.push_sql("DISTINCT ");
        }
        for (index, item) in statement.projection.iter().enumerate() {
            if index != 0 {
                self.push_sql(", ");
            }
            match item {
                SelectItem::Column { table, column } => self.push_qualified(table, column),
                SelectItem::Expression(expr) => self.write_expr(expr),
            }
        }
        self.push_sql(" FROM ");
        self.push_identifier(statement.table);
        for join in &statement.joins {
            self.push_sql(" INNER JOIN ");
            self.push_identifier(join.table);
            self.push_sql(" ON ");
            self.push_qualified(join.left_table, join.left_column);
            self.push_sql(" = ");
            self.push_qualified(join.right_table, join.right_column);
        }
        self.write_where(&statement.filters);

        if !statement.group_by.is_empty() {
            self.push_sql(" GROUP BY ");
            for (index, expr) in statement.group_by.iter().enumerate() {
                if index != 0 {
                    self.push_sql(", ");
                }
                self.write_expr(expr);
            }
        }

        if let Some(having) = &statement.having {
            self.push_sql(" HAVING ");
            self.write_expr(having);
        }

        if !statement.order_by.is_empty() {
            self.push_sql(" ORDER BY ");
            for (index, term) in statement.order_by.iter().enumerate() {
                if index != 0 {
                    self.push_sql(", ");
                }
                self.write_expr(&term.expr);
                self.push_sql(if term.descending { " DESC" } else { " ASC" });
            }
        }

        if let Some(limit) = statement.limit {
            self.push_sql(" LIMIT ");
            self.push_sql(&limit.to_string());
        }
        if let Some(offset) = statement.offset {
            self.push_sql(" OFFSET ");
            self.push_sql(&offset.to_string());
        }
    }

    /// Renders a `SELECT COUNT(*)` over a statement's table and filters.
    pub fn write_count(&mut self, statement: &SelectStatement) {
        self.push_sql("SELECT COUNT(*) FROM ");
        self.push_identifier(statement.table);
        self.write_where(&statement.filters);
    }

    /// Renders an existence check over a statement's table and filters.
    pub fn write_exists(&mut self, statement: &SelectStatement) {
        self.push_sql("SELECT EXISTS(SELECT 1 FROM ");
        self.push_identifier(statement.table);
        self.write_where(&statement.filters);
        self.sql.push(')');
    }

    /// Renders an `INSERT` statement.
    pub fn write_insert(&mut self, statement: &InsertStatement) {
        self.push_sql("INSERT INTO ");
        self.push_identifier(statement.table);
        self.push_sql(" (");
        for (index, column) in statement.columns.iter().enumerate() {
            if index != 0 {
                self.push_sql(", ");
            }
            self.push_identifier(column);
        }
        self.push_sql(") VALUES ");
        for (row_index, row) in statement.rows.iter().enumerate() {
            if row_index != 0 {
                self.push_sql(", ");
            }
            self.sql.push('(');
            for (value_index, value) in row.iter().enumerate() {
                if value_index != 0 {
                    self.push_sql(", ");
                }
                self.push_bind(value.clone());
            }
            self.sql.push(')');
        }
        if !statement.returning.is_empty() {
            self.push_sql(" RETURNING ");
            for (index, column) in statement.returning.iter().enumerate() {
                if index != 0 {
                    self.push_sql(", ");
                }
                self.push_identifier(column);
            }
        }
    }

    /// Renders an `UPDATE` statement.
    pub fn write_update(&mut self, statement: &UpdateStatement) {
        self.push_sql("UPDATE ");
        self.push_identifier(statement.table);
        self.push_sql(" SET ");
        for (index, assignment) in statement.assignments.iter().enumerate() {
            if index != 0 {
                self.push_sql(", ");
            }
            self.push_identifier(assignment.column);
            self.push_sql(" = ");
            self.push_bind(assignment.value.clone());
        }
        self.write_where(&statement.filters);
    }

    /// Renders a `DELETE` statement.
    pub fn write_delete(&mut self, statement: &DeleteStatement) {
        self.push_sql("DELETE FROM ");
        self.push_identifier(statement.table);
        self.write_where(&statement.filters);
    }

    /// Consumes the writer, returning the SQL string and its bound parameters.
    pub fn finish(self) -> (String, Vec<Value>) {
        (self.sql, self.params)
    }
}

/// Renders a `SELECT` statement to SQL and bound parameters.
pub fn render_select(dialect: &dyn Dialect, statement: &SelectStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_select(statement);
    writer.finish()
}

/// Renders a count query to SQL and bound parameters.
pub fn render_count(dialect: &dyn Dialect, statement: &SelectStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_count(statement);
    writer.finish()
}

/// Renders an existence query to SQL and bound parameters.
pub fn render_exists(dialect: &dyn Dialect, statement: &SelectStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_exists(statement);
    writer.finish()
}

/// Renders an `INSERT` statement to SQL and bound parameters.
pub fn render_insert(dialect: &dyn Dialect, statement: &InsertStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_insert(statement);
    writer.finish()
}

/// Renders an `UPDATE` statement to SQL and bound parameters.
pub fn render_update(dialect: &dyn Dialect, statement: &UpdateStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_update(statement);
    writer.finish()
}

/// Renders a `DELETE` statement to SQL and bound parameters.
pub fn render_delete(dialect: &dyn Dialect, statement: &DeleteStatement) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_delete(statement);
    writer.finish()
}

/// Renders a standalone boolean expression to SQL and its bound parameters.
///
/// A convenience over building a [`QueryWriter`] directly, used to render a
/// predicate (such as a `WHERE` clause) in isolation.
pub fn render_expr(dialect: &dyn Dialect, expr: &Expr) -> (String, Vec<Value>) {
    let mut writer = QueryWriter::new(dialect);
    writer.write_expr(expr);
    writer.finish()
}
