use super::super::*;
use super::support::{commit_file, git, repo, request};
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::test]
async fn ready_requires_clean_committed_changes() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("ready"))
        .await
        .expect("create workspace");

    let error = service
        .mark_ready(repo.path(), "ready")
        .await
        .expect_err("unchanged workspace is not ready");
    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));

    std::fs::write(record.worktree_path.join("README.md"), "dirty\n").expect("dirty workspace");
    let error = service
        .mark_ready(repo.path(), "ready")
        .await
        .expect_err("dirty workspace is not ready");
    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));

    git(&record.worktree_path, &["add", "README.md"]);
    git(&record.worktree_path, &["commit", "-qm", "ready change"]);
    let ready = service
        .mark_ready(repo.path(), "ready")
        .await
        .expect("mark ready");
    assert_eq!(ready.state, ManagedWorkspaceState::ReadyToIntegrate);
    assert_eq!(ready.changed_paths, vec![PathBuf::from("README.md")]);
}

#[tokio::test]
async fn integrate_fast_forwards_clean_target_and_cleans_workspace() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("integrate"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "integrated\n",
        "integrated change",
    );
    service
        .mark_ready(repo.path(), "integrate")
        .await
        .expect("mark ready");

    let integrated = service
        .integrate(repo.path(), "integrate")
        .await
        .expect("integrate workspace");

    assert_eq!(integrated.state, ManagedWorkspaceState::Integrated);
    assert!(integrated.integrated_commit.is_some());
    assert!(!record.worktree_path.exists());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        "integrated\n"
    );
}

#[tokio::test]
async fn integrate_refuses_dirty_target_without_mutation() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("blocked"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    service
        .mark_ready(repo.path(), "blocked")
        .await
        .expect("mark ready");
    std::fs::write(repo.path().join("README.md"), "human work\n").expect("dirty target");

    let error = service
        .integrate(repo.path(), "blocked")
        .await
        .expect_err("dirty target must block integration");

    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        "human work\n"
    );
    assert!(record.worktree_path.exists());
    let inspected = service.inspect(repo.path(), "blocked").await.unwrap();
    assert_eq!(inspected.state, ManagedWorkspaceState::ReadyToIntegrate);
}

#[tokio::test]
async fn integrate_refuses_branch_changes_after_ready() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("moved"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    let ready = service
        .mark_ready(repo.path(), "moved")
        .await
        .expect("mark ready");
    assert!(ready.candidate_commit.is_some());
    commit_file(
        &record.worktree_path,
        "README.md",
        "newer\n",
        "newer change",
    );

    let error = service
        .integrate(repo.path(), "moved")
        .await
        .expect_err("moved branch must block integration");

    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README.md")).unwrap(),
        "base\n"
    );
    assert!(record.worktree_path.exists());
}

#[tokio::test]
async fn ready_cannot_be_re_pinned_after_transition() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("pin-once"))
        .await
        .expect("create workspace");
    commit_file(
        &record.worktree_path,
        "README.md",
        "candidate\n",
        "candidate change",
    );
    service
        .mark_ready(repo.path(), "pin-once")
        .await
        .expect("mark ready");

    let error = service
        .mark_ready(repo.path(), "pin-once")
        .await
        .expect_err("ready candidate cannot be re-pinned");

    assert!(matches!(error, ManagedWorkspaceError::Unsafe(_)));
}

#[tokio::test]
async fn changed_paths_include_both_sides_of_rename() {
    let repo = repo();
    let state = TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("rename"))
        .await
        .expect("create workspace");
    std::fs::rename(
        record.worktree_path.join("README.md"),
        record.worktree_path.join("RENAMED.md"),
    )
    .expect("rename fixture");

    let inspected = service
        .inspect(repo.path(), "rename")
        .await
        .expect("inspect renamed workspace");

    assert!(inspected
        .changed_paths
        .contains(&PathBuf::from("README.md")));
    assert!(inspected
        .changed_paths
        .contains(&PathBuf::from("RENAMED.md")));
}
