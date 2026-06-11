//! Native integration with the Tork web framework (the `tork` feature).
//!
//! Converts an [`OrmError`] into a framework [`Error`](tork_core::Error) so a query
//! flows through a handler with `?` and is mapped to an HTTP status. The original
//! error is preserved as the source, so a registered
//! `exception_handler::<OrmError>()` can recover and remap it.
//!
//! Connection failures surface as `503`, a `one()` miss as `404`, and everything
//! else as `500`.

use tork_core::{Error, ErrorKind as HttpKind};

use crate::error::{ErrorKind, OrmError};

impl From<OrmError> for Error {
    fn from(error: OrmError) -> Self {
        let kind = match error.kind() {
            ErrorKind::Connection => HttpKind::ServiceUnavailable,
            ErrorKind::NotFound => HttpKind::NotFound,
            ErrorKind::MultipleFound
            | ErrorKind::Query
            | ErrorKind::Conversion
            | ErrorKind::Configuration => HttpKind::Internal,
        };
        let message = error.to_string();
        Error::new(kind, message).with_source(error)
    }
}
