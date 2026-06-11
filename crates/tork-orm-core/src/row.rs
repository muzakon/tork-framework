//! A backend-agnostic, owned result row.
//!
//! Drivers read each result row into a [`Row`] before it leaves the worker that
//! produced it, so values never borrow from a driver-owned statement. Columns are
//! addressable by name or by index and read into Rust types through [`FromValue`].

use std::sync::Arc;

use crate::error::OrmError;
use crate::value::{FromValue, Value};

/// One row of a query result.
///
/// The column names are shared (`Arc`) across every row of the same result to
/// avoid re-allocating them per row.
///
/// # Examples
///
/// ```
/// use tork_orm_core::{Row, Value};
///
/// let row = Row::new(
///     vec!["id".to_string(), "name".to_string()],
///     vec![Value::Int(1), Value::Text("alice".to_string())],
/// );
/// assert_eq!(row.get::<i64>("id").unwrap(), 1);
/// assert_eq!(row.get::<String>("name").unwrap(), "alice");
/// ```
#[derive(Debug, Clone)]
pub struct Row {
    columns: Arc<[String]>,
    values: Vec<Value>,
}

impl Row {
    /// Builds a row from column names and their values.
    ///
    /// The two slices must have the same length; the names are stored shared so
    /// that callers building many rows should reuse the same [`Arc`] via
    /// [`Row::with_columns`].
    pub fn new(columns: Vec<String>, values: Vec<Value>) -> Self {
        Self {
            columns: Arc::from(columns),
            values,
        }
    }

    /// Builds a row reusing an already-shared set of column names.
    pub fn with_columns(columns: Arc<[String]>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    /// Returns the shared column names, for reuse across sibling rows.
    pub fn columns(&self) -> Arc<[String]> {
        Arc::clone(&self.columns)
    }

    /// Returns the number of columns in this row.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns `true` if the row has no columns.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the raw value at a column index without conversion.
    pub fn value_at(&self, index: usize) -> Option<&Value> {
        self.values.get(index)
    }

    /// Returns the zero-based position of a named column, if present.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column == name)
    }

    /// Reads the value of a named column into `T`.
    ///
    /// # Errors
    ///
    /// Returns an error if the column is absent or its value cannot be converted
    /// to `T`.
    pub fn get<T: FromValue>(&self, name: &str) -> crate::Result<T> {
        let index = self
            .index_of(name)
            .ok_or_else(|| OrmError::conversion(format!("no column named `{name}` in row")))?;
        self.get_index(index)
    }

    /// Reads the value at a column index into `T`.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is out of range or the value cannot be
    /// converted to `T`.
    pub fn get_index<T: FromValue>(&self, index: usize) -> crate::Result<T> {
        let value = self
            .values
            .get(index)
            .ok_or_else(|| OrmError::conversion(format!("column index {index} out of range")))?
            .clone();
        T::from_value(value)
    }
}
