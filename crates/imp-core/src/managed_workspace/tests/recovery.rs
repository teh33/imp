use std::process::Command;

use super::super::*;
use super::support::{commit_file, git, repo, request};
use tempfile::TempDir;

#[tokio::test]
async fn integration_cleanup_can_resume_after_workspace_is_cleaned() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("cleanup-retry"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    service
        .mark_ready(repo.path(), "cleanup-retry")
        .await
        .expect("mark ready");
    std::fs::write(record.worktree_path.join("late.txt"), "late\n").expect("late change");

    let error = service
        .integrate(repo.path(), "cleanup-retry")
        .await
        .expect_err("late change blocks cleanup");
    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));
    let pending = service
        .inspect(repo.path(), "cleanup-retry")
        .await
        .expect("inspect pending cleanup");
    assert_eq!(pending.state, ManagedWorkspaceState::CleanupPending);
    assert!(pending.integrated_commit.is_some());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        "candidate\n"
    );

    std::fs::remove_file(record.worktree_path.join("late.txt")).expect("clean late change");
    let integrated = service
        .integrate(repo.path(), "cleanup-retry")
        .await
        .expect("resume cleanup");
    assert_eq!(integrated.state, ManagedWorkspaceState::Integrated);
    assert!(!record.worktree_path.exists());
}

#[tokio::test]
async fn cleanup_retry_handles_already_removed_worktree() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("removed-retry"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    let ready = service
        .mark_ready(repo.path(), "removed-retry")
        .await
        .expect("mark ready");
    let candidate = ready.candidate_commit.clone().expect("candidate commit");
    git(repo.path(), &["merge", "--ff-only", &candidate]);
    git(
        repo.path(),
        &["worktree", "remove", record.worktree_path.to_str().unwrap()],
    );
    let repo_root = repo.path().canonicalize().expect("canonical repo root");
    service
        .update_record(&repo_root, &record.id, |stored| {
            stored.state = ManagedWorkspaceState::CleanupPending;
            stored.integrated_commit = Some(candidate.clone());
        })
        .expect("record cleanup pending");

    let integrated = service
        .integrate(repo.path(), "removed-retry")
        .await
        .expect("resume branch cleanup");

    assert_eq!(integrated.state, ManagedWorkspaceState::Integrated);
    let output = Command::new("git")
        .current_dir(repo.path())
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", record.branch),
        ])
        .status()
        .expect("inspect managed branch");
    assert!(!output.success());
}

#[tokio::test]
async fn integrating_state_resumes_after_target_was_fast_forwarded() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("integrating-retry"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    let ready = service
        .mark_ready(repo.path(), "integrating-retry")
        .await
        .expect("mark ready");
    let candidate = ready.candidate_commit.clone().expect("candidate commit");
    git(repo.path(), &["merge", "--ff-only", &candidate]);
    let repo_root = repo.path().canonicalize().expect("canonical repo root");
    service
        .update_record(&repo_root, &record.id, |stored| {
            stored.state = ManagedWorkspaceState::Integrating;
        })
        .expect("record integrating checkpoint");

    let integrated = service
        .integrate(repo.path(), "integrating-retry")
        .await
        .expect("resume integration");

    assert_eq!(integrated.state, ManagedWorkspaceState::Integrated);
    assert_eq!(
        integrated.integrated_commit.as_deref(),
        Some(candidate.as_str())
    );
    assert!(!record.worktree_path.exists());
}

#[tokio::test]
async fn discard_resumes_after_worktree_was_already_removed() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("discard-retry"))
        .await
        .expect("create workspace");
    git(
        repo.path(),
        &[
            "worktree",
            "remove",
            "--force",
            record.worktree_path.to_str().unwrap(),
        ],
    );
    let repo_root = repo.path().canonicalize().expect("canonical repo root");
    service
        .update_record(&repo_root, &record.id, |stored| {
            stored.state = ManagedWorkspaceState::CleanupPending;
        })
        .expect("record cleanup pending");

    let cleaned = service
        .discard(repo.path(), "discard-retry", true)
        .await
        .expect("resume discard cleanup");

    assert_eq!(cleaned.state, ManagedWorkspaceState::Cleaned);
}
