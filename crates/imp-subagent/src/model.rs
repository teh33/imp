use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub(crate) const STATE_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid subagent identifier `{0}`")]
    InvalidId(String),
    #[error("unknown subagent `{0}`")]
    NotFound(String),
    #[error("subagent worker is unavailable: {0}")]
    Unavailable(String),
    #[error("subagent protocol error: {0}")]
    Protocol(String),
    #[error("subagent process error: {0}")]
    Process(String),
    #[error("subagent state error: {0}")]
    State(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Success,
    Blocked,
    NeedsInput,
    Failed,
    Cancelled,
}

impl Status {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Success | Self::Blocked | Self::NeedsInput | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPaths {
    pub state: PathBuf,
    pub transcript: PathBuf,
    pub stderr: PathBuf,
    pub session: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    pub parent_id: String,
    pub child_id: String,
    pub cwd: PathBuf,
    pub executable: PathBuf,
    pub worker_executable: PathBuf,
    pub child_args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub socket: PathBuf,
    pub artifacts: ArtifactPaths,
    pub status: Status,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub summary: Option<String>,
    pub diagnostics: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub metadata: Value,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub parent_id: String,
    pub child_id: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub executable: PathBuf,
    pub worker_executable: PathBuf,
    pub child_args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub timeout_seconds: Option<u64>,
    pub metadata: Value,
}
