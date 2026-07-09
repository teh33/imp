use super::super::*;
use super::support::{git, repo, request};

#[tokio::test]
async fn create_registers_owned_worktree_from_dirty_main() {
    let repo = repo();
    std::fs::write(repo.path().join("README.md"), "dirty\n").expect("dirty main");
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);

    let record = service
        .create(repo.path(), request("alpha"))
        .await
        .expect("create workspace");

    assert_eq!(record.state, ManagedWorkspaceState::Active);
    assert!(record.worktree_path.is_dir());
    assert_eq!(record.branch, "imp/workspace/alpha");
    assert_eq!(
        std::fs::read_to_string(record.worktree_path.join("README.md")).unwrap(),
        "base\n"
    );
    assert_eq!(service.list(repo.path()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn create_enforces_live_workspace_limit() {
    let repo = repo();
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 1);
    service
        .create(repo.path(), request("first"))
        .await
        .expect("first workspace");

    let error = service
        .create(repo.path(), request("second"))
        .await
        .expect_err("limit must block second workspace");

    assert!(matches!(error, ManagedWorkspaceError::LimitReached(1)));
}

#[tokio::test]
async fn discard_requires_confirmation_and_cleans_owned_workspace() {
    let repo = repo();
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("discard"))
        .await
        .expect("create workspace");

    let error = service
        .discard(repo.path(), "discard", false)
        .await
        .expect_err("confirmation required");
    assert!(matches!(error, ManagedWorkspaceError::ConfirmationRequired));
    let cleaned = service
        .discard(repo.path(), "discard", true)
        .await
        .expect("discard workspace");

    assert_eq!(cleaned.state, ManagedWorkspaceState::Cleaned);
    assert!(!record.worktree_path.exists());
}

#[tokio::test]
async fn linked_workspace_commands_share_main_registry() {
    let repo = repo();
    let state = tempfile::TempDir::new().expect("state root");
    let service = ManagedWorkspaceService::new(state.path().into(), 4);
    let record = service
        .create(repo.path(), request("linked"))
        .await
        .expect("create workspace");

    let records = service
        .list(&record.worktree_path)
        .await
        .expect("list from managed worktree");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id.as_str(), "linked");

    git(
        repo.path(),
        &[
            "worktree",
            "remove",
            "--force",
            record.worktree_path.to_str().unwrap(),
        ],
    );
}
