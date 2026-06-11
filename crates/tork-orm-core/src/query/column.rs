//! Typed column handles.
//!
//! `#[derive(Model)]` generates one [`Column`] associated constant per field, so a
//! column is used as a value: `User::is_active`. Its comparison methods build
//! [`Expr`] nodes whose right-hand side is a bound parameter, and the column's Rust
//! type constrains what it can be compared against, so a type mismatch is a compile
//! error rather than a run-time surprise.

use std::marker::PhantomData;

use crate::query::ast::OrderItem;
use crate::query::expr::{AggFunc, BinaryOp, Expr};
use crate::query::write::Assignment;
use crate::value::{BindValue, Value};

/// Marker for column types that support numeric aggregates (`sum`, `avg`, `min`,
/// `max`).
pub trait Numeric {}

impl Numeric for i64 {}
impl Numeric for i32 {}
impl Numeric for f64 {}

/// Converts a comparison right-hand side into a bound [`Value`], constrained by the
/// column's type `T`.
///
/// There are exactly two ways to satisfy it: any `T` that is itself a
/// [`BindValue`] (the identity case), and a `&str` when the column type is
/// `String`. Keeping the set small avoids the ambiguity a blanket `Into<T>` bound
/// would create for integer literals, while still letting `column.eq("text")` take
/// a string slice.
pub trait IntoSqlValue<T> {
    /// Lowers `self` into a bound value.
    fn into_sql_value(self) -> Value;
}

impl<T: BindValue> IntoSqlValue<T> for T {
    fn into_sql_value(self) -> Value {
        self.to_value()
    }
}

impl IntoSqlValue<String> for &str {
    fn into_sql_value(self) -> Value {
        Value::Text(self.to_string())
    }
}

/// A typed reference to a model column.
///
/// `M` is the owning model and `T` is the column's Rust type. The handle is
/// zero-sized beyond two `&'static str`s and is `Copy`, so passing it by value is
/// free.
///
/// # Examples
///
/// ```
/// use tork_orm_core::Column;
///
/// struct User;
/// const IS_ACTIVE: Column<User, bool> = Column::new("users", "is_active");
/// let predicate = IS_ACTIVE.eq(true);
/// # let _ = predicate;
/// ```
pub struct Column<M, T> {
    table: &'static str,
    name: &'static str,
    // `fn() -> (M, T)` keeps `Column` `Send`, `Sync`, and `Copy` regardless of the
    // model and column types, which are only used to constrain comparisons.
    _marker: PhantomData<fn() -> (M, T)>,
}

impl<M, T> Column<M, T> {
    /// Creates a column handle for `table`.`name`.
    pub const fn new(table: &'static str, name: &'static str) -> Self {
        Self {
            table,
            name,
            _marker: PhantomData,
        }
    }

    /// Returns the owning table name.
    pub fn table(&self) -> &'static str {
        self.table
    }

    /// Returns the column name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns this column as an expression node.
    pub fn expr(&self) -> Expr {
        Expr::column(self.table, self.name)
    }

    /// `column = value`
    pub fn eq<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Eq, value)
    }

    /// `column <> value`
    pub fn ne<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Ne, value)
    }

    /// `column > value`
    pub fn gt<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Gt, value)
    }

    /// `column >= value`
    pub fn ge<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Ge, value)
    }

    /// `column < value`
    pub fn lt<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Lt, value)
    }

    /// `column <= value`
    pub fn le<V: IntoSqlValue<T>>(self, value: V) -> Expr {
        self.compare(BinaryOp::Le, value)
    }

    /// `column IN (values...)`
    pub fn in_list<V, I>(self, values: I) -> Expr
    where
        V: IntoSqlValue<T>,
        I: IntoIterator<Item = V>,
    {
        let values = values.into_iter().map(IntoSqlValue::into_sql_value).collect();
        Expr::in_list(self.expr(), values)
    }

    /// `column IS NULL`
    pub fn is_null(self) -> Expr {
        Expr::is_null(self.expr(), false)
    }

    /// `column IS NOT NULL`
    pub fn is_not_null(self) -> Expr {
        Expr::is_null(self.expr(), true)
    }

    /// Orders by this column ascending.
    pub fn asc(self) -> OrderItem {
        OrderItem::new(self.expr(), false)
    }

    /// Orders by this column descending.
    pub fn desc(self) -> OrderItem {
        OrderItem::new(self.expr(), true)
    }

    /// Builds an `UPDATE` assignment, `column = value`, for use with
    /// [`QuerySet::update`](crate::QuerySet::update).
    pub fn set<V: IntoSqlValue<T>>(self, value: V) -> Assignment {
        Assignment::new(self.name, value.into_sql_value())
    }

    /// Aliases this column in a projection, `column AS "alias"`.
    pub fn as_(self, alias: &'static str) -> Expr {
        self.expr().as_(alias)
    }

    /// `COUNT(column)` for use in a projection or `HAVING`.
    pub fn count(self) -> Expr {
        Expr::aggregate(AggFunc::Count, self.expr())
    }

    /// Builds a binary comparison against a bound value.
    fn compare<V: IntoSqlValue<T>>(self, op: BinaryOp, value: V) -> Expr {
        Expr::binary(self.expr(), op, Expr::value(value.into_sql_value()))
    }
}

impl<M, T: Numeric> Column<M, T> {
    /// `SUM(column)`.
    pub fn sum(self) -> Expr {
        Expr::aggregate(AggFunc::Sum, self.expr())
    }

    /// `AVG(column)`.
    pub fn avg(self) -> Expr {
        Expr::aggregate(AggFunc::Avg, self.expr())
    }

    /// `MIN(column)`.
    pub fn min(self) -> Expr {
        Expr::aggregate(AggFunc::Min, self.expr())
    }

    /// `MAX(column)`.
    pub fn max(self) -> Expr {
        Expr::aggregate(AggFunc::Max, self.expr())
    }
}

impl<M, T> Clone for Column<M, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M, T> Copy for Column<M, T> {}
