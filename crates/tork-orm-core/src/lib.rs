//! Core runtime for the Tork ORM.
//!
//! This crate holds the backend-neutral pieces of the ORM: the [`Value`] type that
//! crosses the driver boundary, the owned [`Row`] returned from queries, the
//! [`dialect`] abstraction that makes SQL generation backend-specific, the database
//! [`driver`]s, and the [`Database`] handle and [`Executor`] trait used to run SQL.
//!
//! End users do not depend on this crate directly; they depend on `tork-orm`, which
//! re-exports this runtime together with the derive macros.
#![forbid(unsafe_code)]

pub mod dialect;
pub mod driver;
pub mod query;

pub mod preload;

#[cfg(feature = "tork")]
mod bridge;

mod database;
mod error;
mod executor;
mod model;
mod relation;
mod row;
mod value;

pub use database::Database;
pub use dialect::SqlType;
pub use error::{ErrorKind, OrmError, Result};
pub use executor::Executor;
pub use model::{ColumnDef, ForeignKeyDef, FromRow, Model};
pub use preload::{Preloaded, Preloader};
pub use query::ast::{Join, OrderItem, SelectItem, SelectStatement};
pub use query::column::{Column, IntoSqlValue, Numeric};
pub use query::expr::{AggFunc, BinaryOp, Expr, LogicalOp};
pub use query::projection::{ExprTuple, IntoExpr, IntoSelectItem, Projection};
pub use query::write::{Assignment, DeleteStatement, InsertStatement, UpdateStatement};
pub use query::QuerySet;
pub use relation::{Relation, RelationKind};
pub use row::Row;
pub use value::{BindValue, FromValue, Value};

pub use driver::ExecuteResult;
