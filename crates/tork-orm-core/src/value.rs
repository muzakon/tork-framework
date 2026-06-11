//! The dialect-agnostic value type that crosses the database driver boundary.
//!
//! Every literal that appears in a query is carried as a [`Value`] and bound as a
//! parameter, never interpolated into SQL text. Each driver converts between
//! [`Value`] and its own native representation, so the rest of the ORM stays free
//! of backend-specific types.

use time::OffsetDateTime;

/// A single database value, independent of any backend.
///
/// This is the common currency between the query layer and a driver: query
/// parameters are lowered into `Value`s, and result columns are read back as
/// `Value`s before being converted into Rust types via [`FromValue`].
///
/// # Examples
///
/// ```
/// use tork_orm_core::Value;
///
/// let name = Value::Text("alice".to_string());
/// let active = Value::Bool(true);
/// assert!(matches!(name, Value::Text(_)));
/// assert!(matches!(active, Value::Bool(true)));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A boolean, stored as an integer `0` or `1` by backends without a native type.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// A 64-bit floating point number.
    Real(f64),
    /// UTF-8 text.
    Text(String),
    /// Raw bytes.
    Blob(Vec<u8>),
    /// A timestamp, rendered to and parsed from RFC 3339 text by default.
    Timestamp(OffsetDateTime),
}

impl Value {
    /// Returns `true` if this value is [`Value::Null`].
    ///
    /// # Examples
    ///
    /// ```
    /// use tork_orm_core::Value;
    ///
    /// assert!(Value::Null.is_null());
    /// assert!(!Value::Int(1).is_null());
    /// ```
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

/// Converts a Rust value into a bound [`Value`].
///
/// Implemented for the primitive column types the ORM understands. A blanket
/// implementation maps `Option<T>` so that `None` becomes [`Value::Null`].
pub trait BindValue {
    /// Lowers this value into a bound parameter.
    fn to_value(&self) -> Value;
}

impl BindValue for Value {
    fn to_value(&self) -> Value {
        self.clone()
    }
}

impl BindValue for bool {
    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }
}

impl BindValue for i64 {
    fn to_value(&self) -> Value {
        Value::Int(*self)
    }
}

impl BindValue for i32 {
    fn to_value(&self) -> Value {
        Value::Int(i64::from(*self))
    }
}

impl BindValue for f64 {
    fn to_value(&self) -> Value {
        Value::Real(*self)
    }
}

impl BindValue for String {
    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }
}

impl BindValue for &str {
    fn to_value(&self) -> Value {
        Value::Text((*self).to_string())
    }
}

impl BindValue for Vec<u8> {
    fn to_value(&self) -> Value {
        Value::Blob(self.clone())
    }
}

impl BindValue for OffsetDateTime {
    fn to_value(&self) -> Value {
        Value::Timestamp(*self)
    }
}

impl<T: BindValue> BindValue for Option<T> {
    fn to_value(&self) -> Value {
        match self {
            Some(inner) => inner.to_value(),
            None => Value::Null,
        }
    }
}

/// Converts a [`Value`] read from a result row into a Rust type.
///
/// Returns an [`Err`](crate::OrmError) when the stored value cannot be coerced to
/// the requested type (for example a `NULL` read into a non-optional field).
pub trait FromValue: Sized {
    /// Attempts to read this type from a database value.
    fn from_value(value: Value) -> crate::Result<Self>;
}

/// Builds a type-mismatch error for a failed [`FromValue`] conversion.
fn mismatch(expected: &str, value: &Value) -> crate::OrmError {
    crate::OrmError::conversion(format!(
        "cannot read {expected} from value `{value:?}`"
    ))
}

impl FromValue for Value {
    fn from_value(value: Value) -> crate::Result<Self> {
        Ok(value)
    }
}

impl FromValue for bool {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Bool(b) => Ok(b),
            Value::Int(i) => Ok(i != 0),
            other => Err(mismatch("bool", &other)),
        }
    }
}

impl FromValue for i64 {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Int(i) => Ok(i),
            Value::Bool(b) => Ok(i64::from(b)),
            other => Err(mismatch("i64", &other)),
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: Value) -> crate::Result<Self> {
        let wide = i64::from_value(value)?;
        i32::try_from(wide).map_err(|_| crate::OrmError::conversion("integer out of range for i32"))
    }
}

impl FromValue for f64 {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Real(r) => Ok(r),
            Value::Int(i) => Ok(i as f64),
            other => Err(mismatch("f64", &other)),
        }
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Text(s) => Ok(s),
            other => Err(mismatch("String", &other)),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Blob(b) => Ok(b),
            Value::Text(s) => Ok(s.into_bytes()),
            other => Err(mismatch("Vec<u8>", &other)),
        }
    }
}

impl FromValue for OffsetDateTime {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Timestamp(ts) => Ok(ts),
            Value::Text(s) => OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
                .map_err(|_| crate::OrmError::conversion("invalid RFC 3339 timestamp")),
            other => Err(mismatch("OffsetDateTime", &other)),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: Value) -> crate::Result<Self> {
        match value {
            Value::Null => Ok(None),
            other => T::from_value(other).map(Some),
        }
    }
}
