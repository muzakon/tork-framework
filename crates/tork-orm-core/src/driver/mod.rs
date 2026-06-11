//! Database drivers: the bridge between the backend-neutral query layer and a
//! concrete database engine.
//!
//! A driver owns its connections, runs SQL with bound parameters off the async
//! runtime, and reads results back into [`Row`](crate::Row)s. This phase ships a
//! single SQLite driver; future backends add a sibling module behind their feature.

#[cfg(feature = "sqlite")]
pub mod sqlite;

/// The outcome of a statement that does not return rows.
///
/// Returned by execute-style calls (`INSERT` / `UPDATE` / `DELETE` and DDL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExecuteResult {
    /// The number of rows the statement changed.
    pub rows_affected: u64,
    /// The row id of the most recent insert on this connection.
    ///
    /// Meaningful for tables with an auto-incrementing integer primary key;
    /// `0` when no insert produced a row id.
    pub last_insert_rowid: i64,
}
