use std::path::Path;

use chrono::Utc;

use super::git;
use super::{
    ManagedWorkspaceError, ManagedWorkspaceId, ManagedWorkspaceRecord, ManagedWorkspaceResult,
    ManagedWorkspaceService, ManagedWorkspaceState,
};

impl ManagedWorkspaceService {
    pub async fn integrate(
        &self,
        cwd: &Path,
        id: &str,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let repo_root = git::main_worktree(cwd).await?;
        let id =
            ManagedWorkspaceId::parse(id.to_string()).map_err(ManagedWorkspaceError::Invalid)?;
        let record = self.record(&repo_root, &id)?;
        let candidate_commit = candidate_commit(&record)?;
        match record.state {
            ManagedWorkspaceState::ReadyToIntegrate => {
                self.validate_candidate(&record, candidate_commit).await?;
                validate_target(&record).await?;
                self.update_record(&repo_root, &id, |stored| {
                    stored.state = ManagedWorkspaceState::Integrating;
                    stored.updated_at = Utc::now().to_rfc3339();
                    stored.diagnostic = None;
                })?;
            }
            ManagedWorkspaceState::Integrating => {
                validate_target(&record).await?;
                if record.worktree_path.exists() {
                    self.validate_candidate(&record, candidate_commit).await?;
                }
            }
            ManagedWorkspaceState::CleanupPending if record.integrated_commit.is_some() => {
                let integrated_commit = record.integrated_commit.clone().expect("checked above");
                return self
                    .finish_integration(&repo_root, &id, &record, integrated_commit)
                    .await;
            }
            _ => {
                return Err(ManagedWorkspaceError::Unsafe(
                    "workspace must be marked ready before integration".into(),
                ));
            }
        }
        let target_head = git::head_commit(&record.main_worktree).await?;
        let integrated_commit =
            if git::contains_commit(&record.main_worktree, candidate_commit, &target_head).await? {
                candidate_commit.to_string()
            } else {
                match git::fast_forward(&record.main_worktree, candidate_commit).await {
                    Ok(_) => candidate_commit.to_string(),
                    Err(error) => {
                        self.mark_diagnostic(
                            &repo_root,
                            &id,
                            ManagedWorkspaceState::ReadyToIntegrate,
                            error.to_string(),
                        )?;
                        return Err(error);
                    }
                }
            };
        self.finish_integration(&repo_root, &id, &record, integrated_commit)
            .await
    }

    async fn validate_candidate(
        &self,
        record: &ManagedWorkspaceRecord,
        candidate_commit: &str,
    ) -> ManagedWorkspaceResult<()> {
        self.validate_live_ownership(record).await?;
        if git::head_commit(&record.worktree_path).await? != candidate_commit {
            return Err(ManagedWorkspaceError::Unsafe(
                "workspace branch changed after it was marked ready".into(),
            ));
        }
        Ok(())
    }

    async fn finish_integration(
        &self,
        repo_root: &Path,
        id: &ManagedWorkspaceId,
        record: &ManagedWorkspaceRecord,
        integrated_commit: String,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        let cleanup = async {
            if record.worktree_path.exists() {
                if !git::status_paths(&record.worktree_path).await?.is_empty() {
                    return Err(ManagedWorkspaceError::Unsafe(
                        "managed workspace changed during integration cleanup".into(),
                    ));
                }
                git::remove(repo_root, &record.worktree_path, false).await?;
            }
            git::delete_branch_if_matches(repo_root, &record.branch, &integrated_commit).await
        }
        .await;
        let now = Utc::now().to_rfc3339();
        match cleanup {
            Ok(()) => self.update_record(repo_root, id, |stored| {
                stored.state = ManagedWorkspaceState::Integrated;
                stored.updated_at = now.clone();
                stored.integrated_at = Some(now);
                stored.integrated_commit = Some(integrated_commit);
                stored.clean = true;
                stored.diagnostic = None;
            }),
            Err(error) => {
                self.update_record(repo_root, id, |stored| {
                    stored.state = ManagedWorkspaceState::CleanupPending;
                    stored.updated_at = now.clone();
                    stored.integrated_at = Some(now);
                    stored.integrated_commit = Some(integrated_commit);
                    stored.diagnostic = Some(error.to_string());
                })?;
                Err(error)
            }
        }
    }
}

fn candidate_commit(record: &ManagedWorkspaceRecord) -> ManagedWorkspaceResult<&str> {
    record.candidate_commit.as_deref().ok_or_else(|| {
        ManagedWorkspaceError::Unsafe("ready workspace is missing its candidate commit".into())
    })
}

async fn validate_target(record: &ManagedWorkspaceRecord) -> ManagedWorkspaceResult<()> {
    let branch = git::current_branch(&record.main_worktree).await?;
    if branch != record.target_branch {
        return Err(ManagedWorkspaceError::Unsafe(format!(
            "integration target is on branch {branch}, expected {}",
            record.target_branch
        )));
    }
    if !git::status_paths(&record.main_worktree).await?.is_empty() {
        return Err(ManagedWorkspaceError::Unsafe(
            "integration target worktree is dirty".into(),
        ));
    }
    Ok(())
}
