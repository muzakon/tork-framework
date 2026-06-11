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
//! # Type safety
//!
//! A column is typed on its Rust type, so comparing it against an incompatible
//! value is a compile error rather than a run-time failure:
//!
//! ```compile_fail
//! use tork_orm::prelude::*;
//!
//! #[derive(Model)]
//! #[table(name = "users")]
//! struct User {
//!     #[field(primary_key, auto)]
//!     id: i64,
//!     is_active: bool,
//! }
//!
//! // `is_active` is a bool column; comparing it to a string does not compile.
//! let _ = User::is_active.eq("not a bool");
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
        Assignment, BindValue, Column, ColumnDef, Database, ErrorKind, Executor, Expr,
        ForeignKeyDef, FromRow, FromValue, Model, OrderItem, OrmError, Preloaded, QuerySet,
        Relation, RelationKind, Result, Row, SqlType, Value,
    };
    // The derive and attribute macros (`Model`, `relations`).
    pub use tork_orm_macros::*;
}
