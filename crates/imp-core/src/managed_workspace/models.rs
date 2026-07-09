use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ManagedWorkspaceId(String);

impl ManagedWorkspaceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 80
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err("workspace id must contain 1-80 letters, digits, '-' or '_'".into());
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ManagedWorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedWorkspaceState {
    Provisioning,
    Active,
    ReadyToIntegrate,
    Retained,
    Integrating,
    Integrated,
    CleanupPending,
    Orphaned,
    Cleaned,
}

impl ManagedWorkspaceState {
    pub fn counts_toward_limit(self) -> bool {
        matches!(
            self,
            Self::Provisioning
                | Self::Active
                | Self::ReadyToIntegrate
                | Self::Retained
                | Self::Integrating
                | Self::CleanupPending
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedWorkspaceRecord {
    pub id: ManagedWorkspaceId,
    pub run_id: String,
    pub task: Option<String>,
    pub repo_root: PathBuf,
    pub main_worktree: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub target_branch: String,
    pub base_ref: String,
    pub base_commit: String,
    pub state: ManagedWorkspaceState,
    pub created_at: String,
    pub updated_at: String,
    pub clean: bool,
    pub changed_paths: Vec<PathBuf>,
    pub diagnostic: Option<String>,
    pub candidate_commit: Option<String>,
    pub integrated_commit: Option<String>,
    pub integrated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedWorkspaceRegistry {
    pub schema_version: u32,
    pub repo_root: PathBuf,
    pub workspaces: Vec<ManagedWorkspaceRecord>,
}

impl ManagedWorkspaceRegistry {
    pub fn empty(repo_root: PathBuf) -> Self {
        Self {
            schema_version: 1,
            repo_root,
            workspaces: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedWorkspaceFinding {
    pub code: String,
    pub message: String,
    pub workspace_id: Option<ManagedWorkspaceId>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedWorkspaceDoctorReport {
    pub registry_path: PathBuf,
    pub records: Vec<ManagedWorkspaceRecord>,
    pub findings: Vec<ManagedWorkspaceFinding>,
}

impl ManagedWorkspaceDoctorReport {
    pub fn is_healthy(&self) -> bool {
        self.findings.is_empty()
    }
}
