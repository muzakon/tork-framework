//! The boolean expression AST used in `WHERE` and `HAVING` clauses.
//!
//! Expressions are built by the typed [`Column`](crate::Column) handles and the
//! query builder's filter combinators, then rendered to SQL plus an ordered list
//! of bound parameters by a [`Dialect`](crate::dialect::Dialect). Keeping the AST
//! backend-neutral is what lets one set of queries target any dialect.

use crate::query::ast::OrderItem;
use crate::value::{BindValue, Value};

/// An aggregate function applied to an expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    /// `COUNT`
    Count,
    /// `SUM`
    Sum,
    /// `AVG`
    Avg,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
}

impl AggFunc {
    /// Returns the SQL name of this function.
    pub fn as_sql(self) -> &'static str {
        match self {
            AggFunc::Count => "COUNT",
            AggFunc::Sum => "SUM",
            AggFunc::Avg => "AVG",
            AggFunc::Min => "MIN",
            AggFunc::Max => "MAX",
        }
    }
}

/// A binary comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `<`
    Lt,
    /// `<=`
    Le,
}

impl BinaryOp {
    /// Returns the SQL spelling of this operator.
    pub fn as_sql(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
        }
    }
}

/// A logical connective joining several expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    /// `AND`
    And,
    /// `OR`
    Or,
}

impl LogicalOp {
    /// Returns the SQL spelling of this connective.
    pub fn as_sql(self) -> &'static str {
        match self {
            LogicalOp::And => "AND",
            LogicalOp::Or => "OR",
        }
    }
}

/// A boolean expression over columns and bound values.
#[derive(Debug, Clone)]
pub enum Expr {
    /// A qualified column reference, `"table"."column"`.
    Column {
        /// The owning table.
        table: &'static str,
        /// The column name.
        column: &'static str,
    },
    /// A bound literal value.
    Value(Value),
    /// A binary comparison between two expressions.
    Binary {
        /// The left operand.
        left: Box<Expr>,
        /// The operator.
        op: BinaryOp,
        /// The right operand.
        right: Box<Expr>,
    },
    /// Several expressions joined by `AND` or `OR`.
    Logical {
        /// The connective.
        op: LogicalOp,
        /// The joined expressions.
        items: Vec<Expr>,
    },
    /// The negation of an expression.
    Not(Box<Expr>),
    /// A membership test, `expr IN (values...)`.
    InList {
        /// The tested expression.
        expr: Box<Expr>,
        /// The candidate values.
        values: Vec<Value>,
    },
    /// A null test, `expr IS [NOT] NULL`.
    IsNull {
        /// The tested expression.
        expr: Box<Expr>,
        /// Whether the test is negated (`IS NOT NULL`).
        negated: bool,
    },
    /// An aggregate over an expression, `FUNC(expr)`.
    Aggregate {
        /// The aggregate function.
        func: AggFunc,
        /// The aggregated expression.
        arg: Box<Expr>,
    },
    /// `COUNT(*)`.
    CountStar,
    /// An aliased expression, `expr AS "alias"`.
    Alias {
        /// The aliased expression.
        expr: Box<Expr>,
        /// The output name.
        alias: &'static str,
    },
}

impl Expr {
    /// Builds a column reference.
    pub fn column(table: &'static str, column: &'static str) -> Self {
        Expr::Column { table, column }
    }

    /// Builds a bound value.
    pub fn value(value: Value) -> Self {
        Expr::Value(value)
    }

    /// Builds a binary comparison.
    pub fn binary(left: Expr, op: BinaryOp, right: Expr) -> Self {
        Expr::Binary {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    /// Joins expressions with `AND`.
    ///
    /// An empty input is the always-true expression.
    pub fn all(items: impl IntoIterator<Item = Expr>) -> Self {
        Expr::Logical {
            op: LogicalOp::And,
            items: items.into_iter().collect(),
        }
    }

    /// Joins expressions with `OR`.
    ///
    /// An empty input is the always-false expression.
    pub fn any(items: impl IntoIterator<Item = Expr>) -> Self {
        Expr::Logical {
            op: LogicalOp::Or,
            items: items.into_iter().collect(),
        }
    }

    /// Negates an expression.
    // This is a constructor taking an expression, not a `self` method, so it does
    // not conflict with `std::ops::Not` in practice; the name mirrors `all`/`any`.
    #[allow(clippy::should_implement_trait)]
    pub fn not(expr: Expr) -> Self {
        Expr::Not(Box::new(expr))
    }

    /// Builds a membership test.
    pub fn in_list(expr: Expr, values: Vec<Value>) -> Self {
        Expr::InList {
            expr: Box::new(expr),
            values,
        }
    }

    /// Builds a null test (`negated` selects `IS NOT NULL`).
    pub fn is_null(expr: Expr, negated: bool) -> Self {
        Expr::IsNull {
            expr: Box::new(expr),
            negated,
        }
    }

    /// Builds an aggregate over an expression.
    pub fn aggregate(func: AggFunc, arg: Expr) -> Self {
        Expr::Aggregate {
            func,
            arg: Box::new(arg),
        }
    }

    /// Aliases this expression, `expr AS "alias"`.
    ///
    /// Used in a projection so the output column has a stable name to map onto a
    /// [`QueryResult`](crate::FromRow) field.
    pub fn as_(self, alias: &'static str) -> Self {
        Expr::Alias {
            expr: Box::new(self),
            alias,
        }
    }

    /// Builds a comparison of this expression against a bound value.
    ///
    /// Handy for `HAVING` over an aggregate, for example
    /// `Post::id.count().gt(3)`.
    fn compare(self, op: BinaryOp, value: impl BindValue) -> Self {
        Expr::binary(self, op, Expr::Value(value.to_value()))
    }

    /// `expr = value`
    pub fn eq(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Eq, value)
    }

    /// `expr <> value`
    pub fn ne(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Ne, value)
    }

    /// `expr > value`
    pub fn gt(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Gt, value)
    }

    /// `expr >= value`
    pub fn ge(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Ge, value)
    }

    /// `expr < value`
    pub fn lt(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Lt, value)
    }

    /// `expr <= value`
    pub fn le(self, value: impl BindValue) -> Self {
        self.compare(BinaryOp::Le, value)
    }

    /// Orders by this expression ascending.
    pub fn asc(self) -> OrderItem {
        OrderItem::new(self, false)
    }

    /// Orders by this expression descending.
    pub fn desc(self) -> OrderItem {
        OrderItem::new(self, true)
    }
}
