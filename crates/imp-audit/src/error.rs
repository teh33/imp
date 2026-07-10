use thiserror::Error;

/// Result type for imp audit operations.
pub type Result<T> = std::result::Result<T, AuditError>;

/// Errors produced by audit configuration, parsing, and reporting.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuditError {
    /// Audit configuration could not be parsed.
    #[error("failed to parse audit config: {0}")]
    ConfigParse(String),
    /// Audit configuration is syntactically valid but semantically invalid.
    #[error("invalid audit config: {0}")]
    ConfigValidation(String),
    /// A referenced check or requirement was not found.
    #[error("unknown audit item `{0}`")]
    UnknownItem(String),
}
