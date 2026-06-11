//! Tork ORM — a Tortoise-style async ORM for Rust, native to the Tork web framework.
//!
//! This is the facade crate: the single crate end users depend on. It re-exports
//! the runtime from `tork-orm-core` and the derive macros from `tork-orm-macros`.
//! Queries are expressed through typed column handles rather than raw strings, and
//! a dialect-agnostic core keeps the query model independent of any one database.
//!
//! # Example
//!
//! ```no_run
//! use tork_orm::prelude::*;
//!
//! # async fn run() -> tork_orm::Result<()> {
//! let db = Database::connect("sqlite://app.db", 4).await?;
//! db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)".into(), vec![])
//!     .await?;
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]

pub use tork_orm_core::*;
pub use tork_orm_macros::*;

/// The common imports for working with the ORM.
///
/// Bringing `tork_orm::prelude::*` into scope pulls in the database handle, the
/// value and row types, and the error type. Later commits add the model and query
/// types here.
pub mod prelude {
    pub use crate::{
        BindValue, ColumnDef, Database, ErrorKind, Executor, ForeignKeyDef, FromRow, FromValue,
        Model, OrmError, Result, Row, SqlType, Value,
    };
}
