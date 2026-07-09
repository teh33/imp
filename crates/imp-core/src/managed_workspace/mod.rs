mod doctor;
mod git;
mod integration;
mod models;
mod registry_ops;
mod service;
mod store;

pub use models::{
    ManagedWorkspaceDoctorReport, ManagedWorkspaceFinding, ManagedWorkspaceId,
    ManagedWorkspaceRecord, ManagedWorkspaceRegistry, ManagedWorkspaceState,
};
pub use service::{CreateManagedWorkspace, ManagedWorkspaceService};

#[derive(Debug, thiserror::Error)]
pub enum ManagedWorkspaceError {
    #[error("managed workspace IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("managed workspace JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("managed workspace Git error: {0}")]
    Git(String),
    #[error("managed workspace registry error: {0}")]
    Registry(String),
    #[error("managed workspace not found: {0}")]
    NotFound(String),
    #[error("invalid managed workspace request: {0}")]
    Invalid(String),
    #[error("managed workspace limit reached ({0})")]
    LimitReached(usize),
    #[error("discard requires explicit confirmation")]
    ConfirmationRequired,
    #[error("unsafe managed workspace operation: {0}")]
    Unsafe(String),
}

pub type ManagedWorkspaceResult<T> = Result<T, ManagedWorkspaceError>;

pub(super) fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests;
