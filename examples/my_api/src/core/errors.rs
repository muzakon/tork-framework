//! Domain errors that convert into framework errors.

/// An error from the data layer.
///
/// Deriving `AppError` generates `From<RepoError> for tork::Error`, so a
/// repository can surface it through `?`. The `#[status(503)]` sets the default
/// response status, and a registered `exception_handler::<RepoError>()` can map
/// it to a tailored response.
#[derive(Debug, tork::AppError)]
#[status(503)]
pub enum RepoError {
    /// The data store could not be reached.
    Unavailable,
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoError::Unavailable => f.write_str("the data store is unavailable"),
        }
    }
}

impl std::error::Error for RepoError {}
