//! The ORM error type.
//!
//! [`OrmError`] is the single error returned across the ORM surface. When the
//! `tork` feature is enabled, `src/bridge.rs` converts it into a framework error
//! so it can flow through handlers with `?` and be mapped by an exception handler.

use std::fmt;

/// The category of an [`OrmError`].
///
/// The kind drives both human-readable messages and, through the framework
/// bridge, the HTTP status a failed query maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Establishing or acquiring a database connection failed.
    Connection,
    /// Executing or preparing a statement failed.
    Query,
    /// A value could not be converted between a Rust type and a database value.
    Conversion,
    /// A query that required exactly one row found none.
    NotFound,
    /// A query that required exactly one row found more than one.
    MultipleFound,
    /// The configuration or database URL was invalid.
    Configuration,
}

impl ErrorKind {
    /// Returns a short, stable label for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Connection => "connection",
            ErrorKind::Query => "query",
            ErrorKind::Conversion => "conversion",
            ErrorKind::NotFound => "not_found",
            ErrorKind::MultipleFound => "multiple_found",
            ErrorKind::Configuration => "configuration",
        }
    }
}

/// An error produced by the ORM.
///
/// # Examples
///
/// ```
/// use tork_orm_core::{ErrorKind, OrmError};
///
/// let err = OrmError::not_found("no user with that id");
/// assert_eq!(err.kind(), ErrorKind::NotFound);
/// ```
#[derive(Debug)]
pub struct OrmError {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl OrmError {
    /// Builds an error of the given kind with a message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Attaches an underlying source error.
    pub fn with_source(
        mut self,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Returns the kind of this error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Builds a [`ErrorKind::Connection`] error.
    pub fn connection(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Connection, message)
    }

    /// Builds a [`ErrorKind::Query`] error.
    pub fn query(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Query, message)
    }

    /// Builds a [`ErrorKind::Conversion`] error.
    pub fn conversion(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conversion, message)
    }

    /// Builds a [`ErrorKind::NotFound`] error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// Builds a [`ErrorKind::MultipleFound`] error.
    pub fn multiple_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::MultipleFound, message)
    }

    /// Builds a [`ErrorKind::Configuration`] error.
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, message)
    }
}

impl fmt::Display for OrmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for OrmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// A specialized result type for ORM operations.
pub type Result<T, E = OrmError> = std::result::Result<T, E>;
