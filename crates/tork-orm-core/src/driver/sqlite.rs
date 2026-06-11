//! The SQLite driver.
//!
//! Connections are pooled and reused: each call checks an idle connection out of
//! the pool, runs the blocking SQLite work on the runtime's blocking thread pool
//! via [`tokio::task::spawn_blocking`], then returns the connection. A semaphore
//! bounds how many connections run concurrently, so the blocking pool is never
//! flooded. Reusing connections preserves SQLite's per-connection prepared
//! statement cache, and a connection survives a failed query (only a panic or a
//! failed open removes it), so the pool neither leaks nor thrashes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::types::{ToSqlOutput, Value as SqliteValue, ValueRef};
use rusqlite::{Connection, ToSql};
use tokio::sync::Semaphore;

use crate::driver::ExecuteResult;
use crate::error::OrmError;
use crate::row::Row;
use crate::value::Value;

/// The busy timeout applied to every connection, in milliseconds.
///
/// With write-ahead logging a brief wait lets concurrent writers serialize
/// instead of failing immediately with `SQLITE_BUSY`.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// How a connection should be opened.
#[derive(Debug, Clone)]
enum Source {
    /// A file-backed database at the given path.
    File(String),
    /// A private in-memory database.
    Memory,
}

/// Shared pool state behind an [`Arc`].
struct Inner {
    source: Source,
    idle: Mutex<Vec<Connection>>,
    semaphore: Semaphore,
    statements: AtomicU64,
}

impl Inner {
    /// Opens and configures a fresh connection.
    fn open(&self) -> crate::Result<Connection> {
        let conn = match &self.source {
            Source::File(path) => Connection::open(path)
                .map_err(|e| OrmError::connection(format!("cannot open `{path}`")).with_source(e))?,
            Source::Memory => Connection::open_in_memory()
                .map_err(|e| OrmError::connection("cannot open in-memory database").with_source(e))?,
        };

        conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))
            .map_err(|e| OrmError::connection("cannot set busy timeout").with_source(e))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| OrmError::connection("cannot enable foreign keys").with_source(e))?;
        if matches!(self.source, Source::File(_)) {
            // Write-ahead logging improves read/write concurrency for file databases.
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| OrmError::connection("cannot enable WAL").with_source(e))?;
        }
        Ok(conn)
    }
}

/// A pool of reusable SQLite connections.
///
/// Cloning a pool is cheap: clones share the same underlying connections and
/// concurrency limit.
#[derive(Clone)]
pub struct SqlitePool {
    inner: Arc<Inner>,
}

impl SqlitePool {
    /// Builds a pool from a database URL and a maximum connection count.
    ///
    /// Accepted URL forms: `sqlite://path/to.db`, `sqlite:path/to.db`, a bare
    /// path, `:memory:`, or `sqlite::memory:`. In-memory databases are private to
    /// a single connection, so the pool is clamped to one connection for them.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is empty or `max_connections` is zero.
    pub fn new(url: &str, max_connections: u32) -> crate::Result<Self> {
        if max_connections == 0 {
            return Err(OrmError::configuration("max_connections must be at least 1"));
        }
        let source = parse_url(url)?;
        let permits = match source {
            Source::Memory => 1,
            Source::File(_) => max_connections as usize,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                source,
                idle: Mutex::new(Vec::new()),
                semaphore: Semaphore::new(permits),
                statements: AtomicU64::new(0),
            }),
        })
    }

    /// Runs a query that returns rows.
    pub async fn fetch_all(&self, sql: String, params: Vec<Value>) -> crate::Result<Vec<Row>> {
        self.with_connection(move |conn| fetch_all(conn, &sql, &params))
            .await
    }

    /// Runs a statement that returns no rows.
    pub async fn execute(&self, sql: String, params: Vec<Value>) -> crate::Result<ExecuteResult> {
        self.with_connection(move |conn| execute(conn, &sql, &params))
            .await
    }

    /// Returns the number of statements run through this pool so far.
    ///
    /// Useful in tests to confirm a query strategy (for example, that preloading
    /// adds one query per relation rather than one per row).
    pub fn statement_count(&self) -> u64 {
        self.inner.statements.load(Ordering::Relaxed)
    }

    /// Drops all idle connections. In-flight calls keep their connection until done.
    pub async fn close(&self) {
        let drained = {
            let mut idle = lock(&self.inner.idle);
            std::mem::take(&mut *idle)
        };
        drop(drained);
    }

    /// Checks out a connection, runs the blocking work off-runtime, and returns it.
    async fn with_connection<F, T>(&self, work: F) -> crate::Result<T>
    where
        F: FnOnce(&mut Connection) -> crate::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.inner.statements.fetch_add(1, Ordering::Relaxed);

        // Bound concurrency before touching the blocking pool.
        let _permit = self
            .inner
            .semaphore
            .acquire()
            .await
            .map_err(|_| OrmError::connection("connection pool is closed"))?;

        let checked_out = lock(&self.inner.idle).pop();
        let inner = Arc::clone(&self.inner);

        let (returned, result) = tokio::task::spawn_blocking(move || {
            let mut conn = match checked_out {
                Some(conn) => conn,
                None => match inner.open() {
                    Ok(conn) => conn,
                    Err(error) => return (None, Err(error)),
                },
            };
            // A query error does not poison the connection, so it goes back to the
            // pool regardless; only a failed open leaves us without one.
            let result = work(&mut conn);
            (Some(conn), result)
        })
        .await
        .map_err(|error| OrmError::query(format!("database worker failed: {error}")))?;

        if let Some(conn) = returned {
            lock(&self.inner.idle).push(conn);
        }
        result
    }
}

/// Locks the idle list, recovering from a poisoned mutex (the guarded data is a
/// plain `Vec` of connections, so a prior panic leaves it in a usable state).
fn lock(mutex: &Mutex<Vec<Connection>>) -> std::sync::MutexGuard<'_, Vec<Connection>> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Parses a database URL into a connection source.
fn parse_url(url: &str) -> crate::Result<Source> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(OrmError::configuration("database url is empty"));
    }

    // Strip an optional `sqlite:` / `sqlite://` scheme.
    let without_scheme = trimmed
        .strip_prefix("sqlite://")
        .or_else(|| trimmed.strip_prefix("sqlite:"))
        .unwrap_or(trimmed);

    if without_scheme == ":memory:" || without_scheme.is_empty() {
        return Ok(Source::Memory);
    }
    Ok(Source::File(without_scheme.to_string()))
}

/// Implements binding so a [`Value`] can be passed as a SQLite parameter.
impl ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let value = match self {
            Value::Null => SqliteValue::Null,
            Value::Bool(b) => SqliteValue::Integer(i64::from(*b)),
            Value::Int(i) => SqliteValue::Integer(*i),
            Value::Real(r) => SqliteValue::Real(*r),
            Value::Text(s) => SqliteValue::Text(s.clone()),
            Value::Blob(b) => SqliteValue::Blob(b.clone()),
            Value::Timestamp(ts) => SqliteValue::Text(format_timestamp(ts)?),
        };
        Ok(ToSqlOutput::Owned(value))
    }
}

/// Formats a timestamp as RFC 3339 text for storage.
fn format_timestamp(ts: &time::OffsetDateTime) -> rusqlite::Result<String> {
    ts.format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

/// Reads a driver-native column value into a backend-neutral [`Value`].
fn read_value(raw: ValueRef<'_>) -> crate::Result<Value> {
    Ok(match raw {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(r) => Value::Real(r),
        ValueRef::Text(bytes) => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| OrmError::conversion("column text is not valid UTF-8"))?;
            Value::Text(text.to_string())
        }
        ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
    })
}

/// Runs a row-returning query against a connection.
fn fetch_all(conn: &mut Connection, sql: &str, params: &[Value]) -> crate::Result<Vec<Row>> {
    let mut statement = conn
        .prepare_cached(sql)
        .map_err(|e| OrmError::query(format!("cannot prepare `{sql}`")).with_source(e))?;

    let column_names: Arc<[String]> = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into();
    let column_count = column_names.len();

    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| OrmError::query("query execution failed").with_source(e))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| OrmError::query("reading a row failed").with_source(e))?
    {
        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            let raw = row
                .get_ref(index)
                .map_err(|e| OrmError::query("reading a column failed").with_source(e))?;
            values.push(read_value(raw)?);
        }
        out.push(Row::with_columns(Arc::clone(&column_names), values));
    }
    Ok(out)
}

/// Runs a non-row-returning statement against a connection.
fn execute(conn: &mut Connection, sql: &str, params: &[Value]) -> crate::Result<ExecuteResult> {
    let affected = conn
        .prepare_cached(sql)
        .map_err(|e| OrmError::query(format!("cannot prepare `{sql}`")).with_source(e))?
        .execute(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| OrmError::query("statement execution failed").with_source(e))?;

    Ok(ExecuteResult {
        rows_affected: affected as u64,
        last_insert_rowid: conn.last_insert_rowid(),
    })
}
