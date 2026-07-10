//! Error types shared across the Powder core.

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

    /// The PCB columnar buffer could not be encoded or decoded.
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

    /// An ORM operation was malformed (unknown table/column/relation,
    /// invalid op JSON, missing where clause, ...).
    #[error("orm error: {0}")]
    Orm(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Error::Database(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_every_variant() {
        assert_eq!(
            Error::Database("boom".into()).to_string(),
            "database error: boom"
        );
        assert_eq!(
            Error::InvalidUrl("nope://x".into()).to_string(),
            "invalid connection url: nope://x"
        );
        assert_eq!(Error::Codec("short".into()).to_string(), "codec error: short");
        assert_eq!(
            Error::Type {
                column: "score".into(),
                message: "not a float".into()
            }
            .to_string(),
            "type error in column `score`: not a float"
        );
        assert_eq!(
            Error::Unsupported("blob".into()).to_string(),
            "unsupported data type: blob"
        );
        assert_eq!(
            Error::Join("panicked".into()).to_string(),
            "background task failed: panicked"
        );
    }

    #[test]
    fn rusqlite_errors_convert_to_database() {
        let e: Error = rusqlite::Error::InvalidQuery.into();
        assert!(matches!(e, Error::Database(_)));
    }
}
