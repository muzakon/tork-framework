//! The [`Executor`] abstraction over things that can run a query.
//!
//! Query terminal and write methods are generic over `Executor` rather than a
//! concrete [`Database`], so the same call site works against a pooled connection
//! today and, in a later phase, against an open transaction with no change to a
//! single query signature. In this phase [`Database`] (and shared references to it)
//! are the only implementors.

use std::future::Future;
use std::sync::Arc;

use crate::database::Database;
use crate::dialect::Dialect;
use crate::driver::ExecuteResult;
use crate::row::Row;
use crate::value::Value;

/// Something that can run SQL with bound parameters.
///
/// The query layer takes `executor: impl Executor`, which lets a caller pass
/// `&db` (a [`Database`]) and, later, a transaction handle interchangeably.
pub trait Executor {
    /// Returns the dialect SQL should be rendered with before running.
    fn dialect(&self) -> &dyn Dialect;

    /// Runs a row-returning query.
    fn fetch_all(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<Vec<Row>>> + Send;

    /// Runs a statement that returns no rows.
    fn execute(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<ExecuteResult>> + Send;
}

impl Executor for Database {
    fn dialect(&self) -> &dyn Dialect {
        Database::dialect(self).as_ref()
    }

    fn fetch_all(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<Vec<Row>>> + Send {
        Database::fetch_all(self, sql, params)
    }

    fn execute(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<ExecuteResult>> + Send {
        Database::execute(self, sql, params)
    }
}

// `Database` is injected into handlers as `Arc<Database>`; implementing `Executor`
// for the `Arc` lets a query take `&db` directly, with no manual deref.
impl Executor for Arc<Database> {
    fn dialect(&self) -> &dyn Dialect {
        <Database as Executor>::dialect(self)
    }

    fn fetch_all(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<Vec<Row>>> + Send {
        <Database as Executor>::fetch_all(self, sql, params)
    }

    fn execute(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<ExecuteResult>> + Send {
        <Database as Executor>::execute(self, sql, params)
    }
}

impl<T> Executor for &T
where
    T: Executor + Sync + ?Sized,
{
    fn dialect(&self) -> &dyn Dialect {
        T::dialect(self)
    }

    fn fetch_all(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<Vec<Row>>> + Send {
        T::fetch_all(*self, sql, params)
    }

    fn execute(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> impl Future<Output = crate::Result<ExecuteResult>> + Send {
        T::execute(*self, sql, params)
    }
}
