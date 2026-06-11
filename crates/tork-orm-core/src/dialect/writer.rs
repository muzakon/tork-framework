//! The shared SQL writer.
//!
//! A [`QueryWriter`] accumulates SQL text and an ordered list of bound parameters,
//! deferring the backend-specific bits (identifier quoting, placeholder spelling)
//! to a [`Dialect`]. The query layer renders the AST through it, so all dialects
//! share one rendering walk and differ only in their primitives.

use crate::dialect::Dialect;
use crate::query::expr::Expr;
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

    /// Consumes the writer, returning the SQL string and its bound parameters.
    pub fn finish(self) -> (String, Vec<Value>) {
        (self.sql, self.params)
    }
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
