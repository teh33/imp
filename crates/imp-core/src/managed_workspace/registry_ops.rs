use std::collections::BTreeSet;
use std::path::Path;

use chrono::Utc;

use super::git;
use super::{
    canonical, ManagedWorkspaceError, ManagedWorkspaceId, ManagedWorkspaceRecord,
    ManagedWorkspaceResult, ManagedWorkspaceService, ManagedWorkspaceState,
};

impl ManagedWorkspaceService {
    pub(super) fn reserve(
        &self,
        repo_root: &Path,
        record: ManagedWorkspaceRecord,
    ) -> ManagedWorkspaceResult<()> {
        self.store.update(repo_root, |registry| {
            let live = registry
                .workspaces
                .iter()
                .filter(|workspace| workspace.state.counts_toward_limit())
                .count();
            if live >= self.max_live {
                return Err(ManagedWorkspaceError::LimitReached(self.max_live));
            }
            if registry.workspaces.iter().any(|item| item.id == record.id) {
                return Err(ManagedWorkspaceError::Invalid(format!(
                    "workspace id already exists: {}",
                    record.id
                )));
            }
            registry.workspaces.push(record);
            Ok(())
        })
    }

    pub(super) fn record(
        &self,
        repo_root: &Path,
        id: &ManagedWorkspaceId,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        self.store
            .load(repo_root)?
            .workspaces
            .into_iter()
            .find(|record| &record.id == id)
            .ok_or_else(|| ManagedWorkspaceError::NotFound(id.to_string()))
    }

    pub(super) fn update_record(
        &self,
        repo_root: &Path,
        id: &ManagedWorkspaceId,
        update: impl FnOnce(&mut ManagedWorkspaceRecord),
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        self.store.update(repo_root, |registry| {
            let record = registry
                .workspaces
                .iter_mut()
                .find(|record| &record.id == id)
                .ok_or_else(|| ManagedWorkspaceError::NotFound(id.to_string()))?;
            update(record);
            Ok(record.clone())
        })
    }

    pub(super) fn mark_diagnostic(
        &self,
        repo_root: &Path,
        id: &ManagedWorkspaceId,
        state: ManagedWorkspaceState,
        diagnostic: String,
    ) -> ManagedWorkspaceResult<ManagedWorkspaceRecord> {
        self.update_record(repo_root, id, |record| {
            record.state = state;
            record.updated_at = Utc::now().to_rfc3339();
            record.diagnostic = Some(diagnostic);
        })
    }

    pub(super) async fn refresh_records(
        &self,
        repo_root: &Path,
    ) -> ManagedWorkspaceResult<Vec<ManagedWorkspaceRecord>> {
        let mut records = self.store.load(repo_root)?.workspaces;
        for record in &mut records {
            self.refresh(record).await?;
        }
        let refreshed = records.clone();
        self.store.update(repo_root, |registry| {
            registry.workspaces = records;
            Ok(())
        })?;
        Ok(refreshed)
    }

    pub(super) fn mark_orphaned(
        &self,
        repo_root: &Path,
        ids: &BTreeSet<ManagedWorkspaceId>,
    ) -> ManagedWorkspaceResult<Vec<ManagedWorkspaceRecord>> {
        self.store.update(repo_root, |registry| {
            for record in &mut registry.workspaces {
                if ids.contains(&record.id) {
                    record.state = ManagedWorkspaceState::Orphaned;
                    record.updated_at = Utc::now().to_rfc3339();
                    record.diagnostic = Some("registered worktree is missing".into());
                }
            }
            Ok(registry.workspaces.clone())
        })
    }

    pub(super) async fn refresh(
        &self,
        record: &mut ManagedWorkspaceRecord,
    ) -> ManagedWorkspaceResult<()> {
        if !record.worktree_path.is_dir()
            || matches!(
                record.state,
                ManagedWorkspaceState::Integrated | ManagedWorkspaceState::Cleaned
            )
        {
            return Ok(());
        }
        record.changed_paths =
            git::changed_paths(&record.worktree_path, &record.base_commit).await?;
        record.clean = git::status_paths(&record.worktree_path).await?.is_empty();
        Ok(())
    }

    pub(super) async fn validate_live_ownership(
        &self,
        record: &ManagedWorkspaceRecord,
    ) -> ManagedWorkspaceResult<()> {
        if matches!(
            record.state,
            ManagedWorkspaceState::Integrated | ManagedWorkspaceState::Cleaned
        ) {
            return Err(ManagedWorkspaceError::Unsafe(
                "workspace no longer has a live worktree".into(),
            ));
        }
        let expected_root = self.store.worktree_root(&record.repo_root);
        if !canonical(&record.worktree_path).starts_with(canonical(&expected_root)) {
            return Err(ManagedWorkspaceError::Unsafe(
                "registered worktree is outside the managed workspace root".into(),
            ));
        }
        let entries = git::list_worktrees(&record.repo_root).await?;
        let entry = entries
            .iter()
            .find(|entry| canonical(&entry.path) == canonical(&record.worktree_path))
            .ok_or_else(|| {
                ManagedWorkspaceError::Unsafe("managed worktree is not registered with Git".into())
            })?;
        if entry.branch.as_deref() != Some(record.branch.as_str()) {
            return Err(ManagedWorkspaceError::Unsafe(
                "managed worktree branch does not match registry".into(),
            ));
        }
        Ok(())
    }
}
