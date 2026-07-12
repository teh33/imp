use std::fs;

use super::*;

fn fixture_spec(fixture: &std::path::Path) -> EvalTaskSpec {
    EvalTaskSpec {
        id: "task".into(),
        repo: String::new(),
        commit: String::new(),
        prompt: "prompt".into(),
        verifier: "true".into(),
        fixture: Some(fixture.into()),
        expectations: Default::default(),
        setup: None,
        max_turns: None,
        timeout_seconds: None,
        verifier_timeout_seconds: None,
    }
}

#[tokio::test]
async fn fixture_checkout_refuses_unowned_git_repository() {
    let root = tempfile::tempdir().unwrap();
    let fixture = root.path().join("fixture");
    let checkout = root.path().join("checkout");
    fs::create_dir_all(&fixture).unwrap();
    fs::write(fixture.join("file.txt"), "fixture").unwrap();
    fs::create_dir_all(&checkout).unwrap();
    run_git(&["init", "-q"], &checkout).await.unwrap();
    fs::write(checkout.join("valuable.txt"), "preserve").unwrap();
    let spec = fixture_spec(&fixture);

    let error = prepare_checkout(&spec, &root.path().join("task.json"), &checkout)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unowned eval checkout"));
    assert_eq!(
        fs::read_to_string(checkout.join("valuable.txt")).unwrap(),
        "preserve"
    );
}

#[tokio::test]
async fn setup_baseline_commits_tracked_changes() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path();
    run_git(&["init", "-q"], repo).await.unwrap();
    fs::write(repo.join("file.txt"), "before").unwrap();
    run_git(&["add", "-A"], repo).await.unwrap();
    commit_with_identity(repo, "base").await.unwrap();
    let before = current_head(repo).await.unwrap();
    fs::write(repo.join("file.txt"), "after").unwrap();

    let baseline = commit_setup_baseline(repo).await.unwrap();

    assert_ne!(baseline, before);
    let status = run_capture("git", &["status", "--porcelain"], repo)
        .await
        .unwrap();
    assert!(status.stdout.is_empty());
}
