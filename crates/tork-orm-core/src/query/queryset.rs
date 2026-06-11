//! The query builder.
//!
//! A [`QuerySet`] accumulates a [`SelectStatement`] through chainable builders and
//! runs it through an [`Executor`](crate::Executor) with a terminal method. The
//! builders mirror a readable, filter-first style: `filter` adds an `AND`
//! predicate, `filter_any` adds an `OR` group, and so on.

use std::marker::PhantomData;

use crate::dialect::{render_count, render_delete, render_exists, render_select, render_update};
use crate::error::OrmError;
use crate::executor::Executor;
use crate::model::{FromRow, Model};
use crate::query::ast::{OrderItem, SelectItem, SelectStatement};
use crate::query::expr::Expr;
use crate::query::write::{Assignment, DeleteStatement, UpdateStatement};

/// A typed query over a model `M`.
///
/// # Examples
///
/// ```no_run
/// use tork_orm_core::{Database, Model};
/// # use tork_orm_core::{Row, Value};
/// # struct User;
/// # impl tork_orm_core::FromRow for User {
/// #     fn from_row(_: &Row) -> tork_orm_core::Result<Self> { Ok(User) }
/// # }
/// # impl Model for User {
/// #     const TABLE: &'static str = "users";
/// #     const COLUMNS: &'static [tork_orm_core::ColumnDef] = &[];
/// #     const PRIMARY_KEY: &'static str = "id";
/// #     fn insert_values(&self) -> Vec<(&'static str, Value)> { vec![] }
/// #     fn primary_key_value(&self) -> Value { Value::Null }
/// # }
/// # async fn run(db: Database) -> tork_orm_core::Result<()> {
/// let users = User::query().limit(20).all(&db).await?;
/// # let _ = users;
/// # Ok(())
/// # }
/// ```
pub struct QuerySet<M: Model> {
    statement: SelectStatement,
    _marker: PhantomData<fn() -> M>,
}

impl<M: Model> QuerySet<M> {
    /// Builds a query selecting every column of `M` from its table.
    pub fn new() -> Self {
        let projection = M::COLUMNS
            .iter()
            .map(|column| SelectItem::Column {
                table: M::TABLE,
                column: column.name,
            })
            .collect();
        Self {
            statement: SelectStatement::new(M::TABLE, projection),
            _marker: PhantomData,
        }
    }

    /// Adds a predicate joined with `AND`.
    pub fn filter(mut self, predicate: Expr) -> Self {
        self.statement.filters.push(predicate);
        self
    }

    /// Adds an `AND (a OR b OR ...)` group from several predicates.
    pub fn filter_any(mut self, predicates: impl IntoIterator<Item = Expr>) -> Self {
        self.statement.filters.push(Expr::any(predicates));
        self
    }

    /// Adds an `AND (a AND b AND ...)` group from several predicates.
    pub fn filter_all(mut self, predicates: impl IntoIterator<Item = Expr>) -> Self {
        self.statement.filters.push(Expr::all(predicates));
        self
    }

    /// Adds an `AND NOT (...)` of a predicate.
    pub fn filter_not(mut self, predicate: Expr) -> Self {
        self.statement.filters.push(Expr::not(predicate));
        self
    }

    /// Joins a related table for filtering or aggregation.
    ///
    /// The join does not load the related rows; use it to filter the queried
    /// model by columns on the related table. A `has_many` join can repeat parent
    /// rows, so pair it with [`distinct`](Self::distinct) when selecting parents.
    /// Only a relation defined on `M` is accepted, so the join is type-checked.
    pub fn join<C>(mut self, relation: crate::relation::Relation<M, C>) -> Self {
        self.statement.joins.push(relation.join_node());
        self
    }

    /// Preloads a related table in a separate, N+1-free query.
    ///
    /// Switches to a [`Preloader`](crate::preload::Preloader); the parents come
    /// back wrapped in [`Preloaded`](crate::preload::Preloaded). Constrain the
    /// related rows on the relation itself with `Relation::filter`/`order_by`.
    pub fn preload<C: Model>(
        self,
        relation: crate::relation::Relation<M, C>,
    ) -> crate::preload::Preloader<M> {
        crate::preload::Preloader::new(self).preload(relation)
    }

    /// Adds an ordering term (build it with `Column::asc`/`Column::desc`).
    pub fn order_by(mut self, term: OrderItem) -> Self {
        self.statement.order_by.push(term);
        self
    }

    /// Limits the number of returned rows.
    pub fn limit(mut self, limit: u64) -> Self {
        self.statement.limit = Some(limit);
        self
    }

    /// Skips the given number of leading rows.
    pub fn offset(mut self, offset: u64) -> Self {
        self.statement.offset = Some(offset);
        self
    }

    /// Returns only distinct rows.
    pub fn distinct(mut self) -> Self {
        self.statement.distinct = true;
        self
    }

    /// Replaces the projection with a tuple of columns and expressions.
    ///
    /// Pair with [`all_as`](Self::all_as) and a `#[derive(QueryResult)]` DTO whose
    /// fields match the projected names (alias aggregates with
    /// [`Expr::as_`](crate::Expr::as_) so they have a stable name).
    pub fn select<P: crate::query::projection::Projection>(mut self, projection: P) -> Self {
        self.statement.projection = projection.into_select_items();
        self
    }

    /// Groups rows by a tuple of columns and expressions.
    pub fn group_by<G: crate::query::projection::ExprTuple>(mut self, group: G) -> Self {
        self.statement.group_by = group.into_exprs();
        self
    }

    /// Restricts grouped rows by a predicate (typically over an aggregate).
    pub fn having(mut self, predicate: Expr) -> Self {
        self.statement.having = Some(predicate);
        self
    }

    /// Returns the statement this query has assembled.
    ///
    /// Useful for inspecting or rendering a query without running it.
    pub fn statement(&self) -> &SelectStatement {
        &self.statement
    }

    /// Runs the query and returns every matching row as `M`.
    pub async fn all(self, executor: impl Executor) -> crate::Result<Vec<M>> {
        self.all_as::<M>(executor).await
    }

    /// Runs the query and maps each row into an arbitrary [`FromRow`] type.
    ///
    /// Only the result type needs naming: `all_as::<UserPostStats>(&db)`.
    pub async fn all_as<T: FromRow>(self, executor: impl Executor) -> crate::Result<Vec<T>> {
        let (sql, params) = render_select(executor.dialect(), &self.statement);
        let rows = executor.fetch_all(sql, params).await?;
        rows.iter().map(T::from_row).collect()
    }

    /// Runs the query and returns the first matching row, if any.
    pub async fn first<E: Executor>(mut self, executor: E) -> crate::Result<Option<M>> {
        self.statement.limit = Some(1);
        let (sql, params) = render_select(executor.dialect(), &self.statement);
        let rows = executor.fetch_all(sql, params).await?;
        match rows.first() {
            Some(row) => M::from_row(row).map(Some),
            None => Ok(None),
        }
    }

    /// Runs the query expecting exactly one row.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::NotFound`](crate::ErrorKind::NotFound) when no row
    /// matches and [`ErrorKind::MultipleFound`](crate::ErrorKind::MultipleFound)
    /// when more than one does.
    pub async fn one<E: Executor>(mut self, executor: E) -> crate::Result<M> {
        // Fetch two rows so a second match can be detected and reported.
        self.statement.limit = Some(2);
        let (sql, params) = render_select(executor.dialect(), &self.statement);
        let rows = executor.fetch_all(sql, params).await?;
        match rows.len() {
            0 => Err(OrmError::not_found(format!(
                "no row in `{}` matched the query",
                M::TABLE
            ))),
            1 => M::from_row(&rows[0]),
            _ => Err(OrmError::multiple_found(format!(
                "more than one row in `{}` matched the query",
                M::TABLE
            ))),
        }
    }

    /// Runs a `COUNT(*)` over the query's filters.
    pub async fn count<E: Executor>(self, executor: E) -> crate::Result<i64> {
        let (sql, params) = render_count(executor.dialect(), &self.statement);
        let rows = executor.fetch_all(sql, params).await?;
        match rows.first() {
            Some(row) => row.get_index::<i64>(0),
            None => Ok(0),
        }
    }

    /// Returns whether any row matches the query's filters.
    pub async fn exists<E: Executor>(self, executor: E) -> crate::Result<bool> {
        let (sql, params) = render_exists(executor.dialect(), &self.statement);
        let rows = executor.fetch_all(sql, params).await?;
        match rows.first() {
            Some(row) => row.get_index::<bool>(0),
            None => Ok(false),
        }
    }

    /// Applies column assignments to every row matching the query's filters.
    ///
    /// Build the assignments with [`Column::set`](crate::Column::set). Returns the
    /// number of rows changed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tork_orm_core::{Database, Model};
    /// # async fn run<M: Model>(db: Database, col: tork_orm_core::Column<M, bool>) -> tork_orm_core::Result<()> {
    /// let changed = M::query().update(&db, [col.set(false)]).await?;
    /// # let _ = changed;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update<E: Executor>(
        self,
        executor: E,
        assignments: impl IntoIterator<Item = Assignment>,
    ) -> crate::Result<u64> {
        let statement = UpdateStatement {
            table: self.statement.table,
            assignments: assignments.into_iter().collect(),
            filters: self.statement.filters,
        };
        let (sql, params) = render_update(executor.dialect(), &statement);
        Ok(executor.execute(sql, params).await?.rows_affected)
    }

    /// Deletes every row matching the query's filters, returning the count removed.
    pub async fn delete<E: Executor>(self, executor: E) -> crate::Result<u64> {
        let statement = DeleteStatement {
            table: self.statement.table,
            filters: self.statement.filters,
        };
        let (sql, params) = render_delete(executor.dialect(), &statement);
        Ok(executor.execute(sql, params).await?.rows_affected)
    }
}

impl<M: Model> Default for QuerySet<M> {
    fn default() -> Self {
        Self::new()
    }
}
