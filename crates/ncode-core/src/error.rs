//! Error types shared across the Ncode core.

use thiserror::Error;

/// The single error type surfaced by the core engine.
///
/// It is intentionally string-backed so it can cross an FFI boundary (napi /
/// PyO3) losslessly without exposing backend-specific error enums to callers.
#[derive(Debug, Error)]
pub enum Error {
    /// The underlying database driver reported a failure.
    #[error("database error: {0}")]
    Database(String),

    /// A connection string could not be understood.
    #[error("invalid connection url: {0}")]
    InvalidUrl(String),

    /// The NCB columnar buffer could not be encoded or decoded.
    #[error("codec error: {0}")]
    Codec(String),

    /// A value did not match the inferred column type.
    #[error("type error in column `{column}`: {message}")]
    Type { column: String, message: String },

    /// A requested data type is not supported by the columnar format.
    #[error("unsupported data type: {0}")]
    Unsupported(String),

    /// A background (spawn_blocking) task failed to join.
    #[error("background task failed: {0}")]
    Join(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Error::Database(value.to_string())
    }
}
