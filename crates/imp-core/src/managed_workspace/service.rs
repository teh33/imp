use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;

use super::git;
use super::store::RegistryStore;
use super::{
    ManagedWorkspaceError, ManagedWorkspaceId, ManagedWorkspaceRecord, ManagedWorkspaceResult,
    ManagedWorkspaceState,
};

const DEFAULT_MAX_LIVE: usize = 4;

pub struct ManagedWorkspaceService {
    pub(super) store: RegistryStore,
    pub(super) max_live: usize,
}

impl ManagedWorkspaceService {
    pub fn global() -> Self {
        Self::new(
            crate::storage::global_root().join("workspaces"),
            DEFAULT_MAX_LIVE,
        )
    }

    pub fn new(root: PathBuf, max_live: usize) -> Self {
        Self {
            store: RegistryStore::new(root),
            max_live,
        }
    }

    pub async fn create(
        &self,
        cwd: &Path,
        request: CreateManagedWorkspace,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let repo_root = git::main_worktree(cwd).await?;
        let main_worktree = repo_root.clone();
        let target_branch = git::current_branch(&main_worktree).await?;
        let id = match request.id {
            Some(id) => ManagedWorkspaceId::parse(id).map_err(ManagedWorkspaceError::Invalid)?,
            None => ManagedWorkspaceId::parse(Uuid::new_v4().simple().to_string())
                .map_err(ManagedWorkspaceError::Invalid)?,
        };
        let base_ref = request.base_ref.unwrap_or_else(|| "HEAD".into());
        let base_commit = git::resolve_commit(&main_worktree, &base_ref).await?;
        let branch = format!("imp/workspace/{id}");
        let worktree_path = self.store.worktree_root(&repo_root).join(id.as_str());
        let now = Utc::now().to_rfc3339();
        let record = ManagedWorkspaceRecord {
            id: id.clone(),
            run_id: request.run_id,
            task: request.task,
            repo_root: repo_root.clone(),
            main_worktree,
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            target_branch,
            base_ref,
            base_commit: base_commit.clone(),
            state: ManagedWorkspaceState::Provisioning,
            created_at: now.clone(),
            updated_at: now,
            clean: true,
            changed_paths: Vec::new(),
            diagnostic: None,
            candidate_commit: None,
            integrated_commit: None,
            integrated_at: None,
        };
        self.reserve(&repo_root, record.clone())?;
        if let Err(error) = git::create(&repo_root, &worktree_path, &branch, &base_commit).await {
            self.mark_diagnostic(
                &repo_root,
                &id,
                ManagedWorkspaceState::Orphaned,
                error.to_string(),
            )?;
            return Err(error);
        }
        self.update_record(&repo_root, &id, |record| {
            record.state = ManagedWorkspaceState::Active;
            record.updated_at = Utc::now().to_rfc3339();
        })
    }

    pub async fn list(&self, cwd: &Path) -> ManagedWorkspaceResult<Vec<ManagedWorkspaceRecord>> {
        let repo_root = git::main_worktree(cwd).await?;
        self.refresh_records(&repo_root).await
    }

    pub async fn inspect(
        &self,
        cwd: &Path,
        id: &str,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let id =
            ManagedWorkspaceId::parse(id.to_string()).map_err(ManagedWorkspaceError::Invalid)?;
        self.list(cwd)
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| ManagedWorkspaceError::NotFound(id.to_string()))
    }

    pub async fn mark_ready(
        &self,
        cwd: &Path,
        id: &str,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let repo_root = git::main_worktree(cwd).await?;
        let id =
            ManagedWorkspaceId::parse(id.to_string()).map_err(ManagedWorkspaceError::Invalid)?;
        let mut record = self.record(&repo_root, &id)?;
        if !matches!(
            record.state,
            ManagedWorkspaceState::Active | ManagedWorkspaceState::Retained
        ) {
            return Err(ManagedWorkspaceError::Unsafe(
                "only an active or retained workspace can be marked ready".into(),
            ));
        }
        self.validate_live_ownership(&record).await?;
        self.refresh(&mut record).await?;
        if !record.clean {
            return Err(ManagedWorkspaceError::Unsafe(
                "workspace must be clean before it can be marked ready".into(),
            ));
        }
        let candidate_commit = git::head_commit(&record.worktree_path).await?;
        if candidate_commit == record.base_commit {
            return Err(ManagedWorkspaceError::Unsafe(
                "workspace has no committed changes beyond its base".into(),
            ));
        }
        if !git::contains_commit(
            &record.worktree_path,
            &record.base_commit,
            &candidate_commit,
        )
        .await?
        {
            return Err(ManagedWorkspaceError::Unsafe(
                "workspace candidate does not descend from its recorded base".into(),
            ));
        }
        self.update_record(&repo_root, &id, |stored| {
            stored.state = ManagedWorkspaceState::ReadyToIntegrate;
            stored.updated_at = Utc::now().to_rfc3339();
            stored.clean = record.clean;
            stored.changed_paths = record.changed_paths.clone();
            stored.diagnostic = None;
            stored.candidate_commit = Some(candidate_commit);
        })
    }

    pub async fn retain(
        &self,
        cwd: &Path,
        id: &str,
        diagnostic: String,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let repo_root = git::main_worktree(cwd).await?;
        let id =
            ManagedWorkspaceId::parse(id.to_string()).map_err(ManagedWorkspaceError::Invalid)?;
        self.update_record(&repo_root, &id, |record| {
            record.state = ManagedWorkspaceState::Retained;
            record.updated_at = Utc::now().to_rfc3339();
            record.diagnostic = Some(diagnostic.clone());
        })
    }

    pub async fn discard(
        &self,
        cwd: &Path,
        id: &str,
        confirmed: bool,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        if !confirmed {
            return Err(ManagedWorkspaceError::ConfirmationRequired);
        }
        let repo_root = git::main_worktree(cwd).await?;
        let id =
            ManagedWorkspaceId::parse(id.to_string()).map_err(ManagedWorkspaceError::Invalid)?;
        let record = self.record(&repo_root, &id)?;
        if record.state == ManagedWorkspaceState::Cleaned {
            return Ok(record);
        }
        if record.worktree_path.exists() {
            self.validate_live_ownership(&record).await?;
            if let Err(error) = git::remove(&repo_root, &record.worktree_path, true).await {
                self.mark_diagnostic(
                    &repo_root,
                    &id,
                    ManagedWorkspaceState::CleanupPending,
                    error.to_string(),
                )?;
                return Err(error);
            }
        } else if record.state != ManagedWorkspaceState::CleanupPending {
            return Err(ManagedWorkspaceError::Unsafe(
                "managed worktree is missing; run workspace doctor before discard".into(),
            ));
        }
        if let Err(error) = git::delete_branch(&repo_root, &record.branch).await {
            self.mark_diagnostic(
                &repo_root,
                &id,
                ManagedWorkspaceState::CleanupPending,
                error.to_string(),
            )?;
            return Err(error);
        }
        self.update_record(&repo_root, &id, |stored| {
            stored.state = ManagedWorkspaceState::Cleaned;
            stored.updated_at = Utc::now().to_rfc3339();
            stored.clean = true;
            stored.changed_paths.clear();
            stored.diagnostic = None;
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateManagedWorkspace {
    pub id: Option<String>,
    pub run_id: String,
    pub task: Option<String>,
    pub base_ref: Option<String>,
}
