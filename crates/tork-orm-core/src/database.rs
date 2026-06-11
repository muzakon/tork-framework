//! The [`Database`] handle: a cloneable entry point to a configured backend.
//!
//! A `Database` pairs a connection pool with the [`Dialect`](crate::dialect::Dialect)
//! that renders queries for it. It is cheap to clone (clones share the pool) and is
//! the value injected into handlers as `Arc<Database>` when the `tork` feature is
//! enabled.

use std::sync::Arc;

use crate::dialect::Dialect;
use crate::driver::ExecuteResult;
use crate::error::OrmError;
use crate::row::Row;
use crate::value::Value;

/// The concrete backend behind a [`Database`].
///
/// One variant per compiled-in driver. This phase ships SQLite only.
#[derive(Clone)]
enum Backend {
    #[cfg(feature = "sqlite")]
    Sqlite(crate::driver::sqlite::SqlitePool),
}

/// A handle to a database.
///
/// # Examples
///
/// ```no_run
/// use tork_orm_core::Database;
///
/// # async fn run() -> tork_orm_core::Result<()> {
/// let db = Database::connect("sqlite://app.db", 4).await?;
/// db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)".into(), vec![]).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Database {
    backend: Backend,
    dialect: Arc<dyn Dialect>,
}

impl Database {
    /// Connects to a database described by `url`, with up to `max_connections`
    /// concurrent connections.
    ///
    /// The backend is chosen from the URL scheme. In this phase only SQLite is
    /// available (`sqlite://...`, a bare path, or `:memory:`).
    ///
    /// # Errors
    ///
    /// Returns an error if the URL names an unsupported backend or the pool cannot
    /// be created.
    pub async fn connect(url: &str, max_connections: u32) -> crate::Result<Self> {
        let scheme = url.split_once(':').map(|(scheme, _)| scheme).unwrap_or("");
        match scheme {
            #[cfg(feature = "sqlite")]
            "sqlite" | "" => {
                let pool = crate::driver::sqlite::SqlitePool::new(url, max_connections)?;
                Ok(Self {
                    backend: Backend::Sqlite(pool),
                    dialect: Arc::new(crate::dialect::SqliteDialect::new()),
                })
            }
            other => Err(OrmError::configuration(format!(
                "no compiled-in backend for url scheme `{other}`"
            ))),
        }
    }

    /// Returns the dialect this database renders queries with.
    pub fn dialect(&self) -> &Arc<dyn Dialect> {
        &self.dialect
    }

    /// Runs a row-returning query with bound parameters.
    pub async fn fetch_all(&self, sql: String, params: Vec<Value>) -> crate::Result<Vec<Row>> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => pool.fetch_all(sql, params).await,
        }
    }

    /// Runs a statement that returns no rows.
    pub async fn execute(
        &self,
        sql: String,
        params: Vec<Value>,
    ) -> crate::Result<ExecuteResult> {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => pool.execute(sql, params).await,
        }
    }

    /// Returns the number of statements run through this database so far.
    ///
    /// Useful in tests to confirm a query strategy (such as preloading adding one
    /// query per relation, not one per row).
    pub fn statement_count(&self) -> u64 {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => pool.statement_count(),
        }
    }

    /// Releases idle connections held by the pool.
    pub async fn close(&self) {
        match &self.backend {
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => pool.close().await,
        }
    }
}
