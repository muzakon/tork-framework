//! Heterogeneous tuples for `select` and `group_by`.
//!
//! `select((a, b, c))` and `group_by((a, b))` accept tuples whose elements are
//! columns or expressions of differing types. These traits convert such a tuple
//! into the list the statement stores, implemented for tuples up to twelve
//! elements.

use crate::query::ast::SelectItem;
use crate::query::column::Column;
use crate::query::expr::Expr;

/// Converts a single projected element into a [`SelectItem`].
pub trait IntoSelectItem {
    /// Lowers `self` into a projection item.
    fn into_select_item(self) -> SelectItem;
}

impl<M, T> IntoSelectItem for Column<M, T> {
    fn into_select_item(self) -> SelectItem {
        SelectItem::Column {
            table: self.table(),
            column: self.name(),
        }
    }
}

impl IntoSelectItem for Expr {
    fn into_select_item(self) -> SelectItem {
        SelectItem::Expression(self)
    }
}

/// Converts a single element into an [`Expr`], for `group_by`.
pub trait IntoExpr {
    /// Lowers `self` into an expression.
    fn into_expr(self) -> Expr;
}

impl<M, T> IntoExpr for Column<M, T> {
    fn into_expr(self) -> Expr {
        self.expr()
    }
}

impl IntoExpr for Expr {
    fn into_expr(self) -> Expr {
        self
    }
}

/// A tuple of projected elements for [`select`](crate::QuerySet::select).
pub trait Projection {
    /// Lowers the tuple into projection items.
    fn into_select_items(self) -> Vec<SelectItem>;
}

/// A tuple of expressions for [`group_by`](crate::QuerySet::group_by).
pub trait ExprTuple {
    /// Lowers the tuple into expressions.
    fn into_exprs(self) -> Vec<Expr>;
}

/// Implements [`Projection`] and [`ExprTuple`] for a tuple of a given arity.
macro_rules! impl_tuples {
    ($($name:ident),+) => {
        impl<$($name),+> Projection for ($($name,)+)
        where
            $($name: IntoSelectItem,)+
        {
            fn into_select_items(self) -> Vec<SelectItem> {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                vec![$($name.into_select_item()),+]
            }
        }

        impl<$($name),+> ExprTuple for ($($name,)+)
        where
            $($name: IntoExpr,)+
        {
            fn into_exprs(self) -> Vec<Expr> {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                vec![$($name.into_expr()),+]
            }
        }
    };
}

impl_tuples!(A);
impl_tuples!(A, B);
impl_tuples!(A, B, C);
impl_tuples!(A, B, C, D);
impl_tuples!(A, B, C, D, E);
impl_tuples!(A, B, C, D, E, F);
impl_tuples!(A, B, C, D, E, F, G);
impl_tuples!(A, B, C, D, E, F, G, H);
impl_tuples!(A, B, C, D, E, F, G, H, I);
impl_tuples!(A, B, C, D, E, F, G, H, I, J);
impl_tuples!(A, B, C, D, E, F, G, H, I, J, K);
impl_tuples!(A, B, C, D, E, F, G, H, I, J, K, L);
