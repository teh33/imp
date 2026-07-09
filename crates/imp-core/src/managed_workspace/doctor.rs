use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::git;
use super::{
    canonical, ManagedWorkspaceDoctorReport, ManagedWorkspaceFinding, ManagedWorkspaceRecord,
    ManagedWorkspaceResult, ManagedWorkspaceService, ManagedWorkspaceState,
};

impl ManagedWorkspaceService {
    pub async fn doctor(&self, cwd: &Path) -> ManagedWorkspaceResult<ManagedWorkspaceDoctorReport> {
        let repo_root = git::main_worktree(cwd).await?;
        let mut records = self.refresh_records(&repo_root).await?;
        let git_worktrees = git::list_worktrees(&repo_root).await?;
        let known: BTreeSet<_> = records
            .iter()
            .filter(|record| is_live_record(record))
            .map(|record| canonical(&record.worktree_path))
            .collect();
        let mut findings = registry_findings(&records, &git_worktrees);
        findings.extend(unmanaged_findings(&git_worktrees, &known));
        findings.extend(overlap_findings(&records));
        let missing: BTreeSet<_> = findings
            .iter()
            .filter(|finding| finding.code == "registered_worktree_missing")
            .filter_map(|finding| finding.workspace_id.clone())
            .collect();
        if !missing.is_empty() {
            records = self.mark_orphaned(&repo_root, &missing)?;
        }
        Ok(ManagedWorkspaceDoctorReport {
            registry_path: self.store.registry_path(&repo_root),
            records,
            findings,
        })
    }
}

fn registry_findings(
    records: &[ManagedWorkspaceRecord],
    git_worktrees: &[git::GitWorktree],
) -> Vec<ManagedWorkspaceFinding> {
    let mut findings = Vec::new();
    for record in records.iter().filter(|record| is_live_record(record)) {
        match git_worktrees
            .iter()
            .find(|entry| canonical(&entry.path) == canonical(&record.worktree_path))
        {
            None if record.state == ManagedWorkspaceState::CleanupPending => {
                findings.push(finding(
                    "cleanup_pending",
                    format!("managed workspace cleanup is incomplete: {}", record.id),
                    Some(record),
                    Some(record.worktree_path.clone()),
                ));
            }
            None => findings.push(finding(
                "registered_worktree_missing",
                format!(
                    "registered worktree is missing: {}",
                    record.worktree_path.display()
                ),
                Some(record),
                Some(record.worktree_path.clone()),
            )),
            Some(entry) if entry.branch.as_deref() != Some(record.branch.as_str()) => {
                findings.push(finding(
                    "branch_mismatch",
                    format!(
                        "managed worktree branch does not match registry: {}",
                        record.id
                    ),
                    Some(record),
                    Some(record.worktree_path.clone()),
                ));
            }
            Some(_) => {}
        }
    }
    findings
}

fn unmanaged_findings(
    git_worktrees: &[git::GitWorktree],
    known: &BTreeSet<PathBuf>,
) -> Vec<ManagedWorkspaceFinding> {
    git_worktrees
        .iter()
        .skip(1)
        .filter(|entry| !known.contains(&canonical(&entry.path)))
        .map(|entry| {
            finding(
                "unmanaged_worktree",
                format!(
                    "Git worktree is not managed by imp: {}",
                    entry.path.display()
                ),
                None,
                Some(entry.path.clone()),
            )
        })
        .collect()
}

fn overlap_findings(records: &[ManagedWorkspaceRecord]) -> Vec<ManagedWorkspaceFinding> {
    let mut owners: BTreeMap<&Path, Vec<&ManagedWorkspaceRecord>> = BTreeMap::new();
    for record in records.iter().filter(|record| is_live_record(record)) {
        for path in &record.changed_paths {
            owners.entry(path).or_default().push(record);
        }
    }
    owners
        .into_iter()
        .filter(|(_, records)| records.len() > 1)
        .map(|(path, records)| {
            let ids = records
                .iter()
                .map(|record| record.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            finding(
                "changed_path_overlap",
                format!("managed workspaces {ids} both change {}", path.display()),
                None,
                Some(path.to_path_buf()),
            )
        })
        .collect()
}

fn is_live_record(record: &ManagedWorkspaceRecord) -> bool {
    !matches!(
        record.state,
        ManagedWorkspaceState::Integrated | ManagedWorkspaceState::Cleaned
    )
}

fn finding(
    code: &str,
    message: String,
    record: Option<&ManagedWorkspaceRecord>,
    path: Option<PathBuf>,
) -> ManagedWorkspaceFinding {
    ManagedWorkspaceFinding {
        code: code.into(),
        message,
        workspace_id: record.map(|record| record.id.clone()),
        path,
    }
}
