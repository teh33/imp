use super::*;
use crate::tools::{CheckpointState, FileCache, FileTracker};
use crate::workflow_review::TurnWorkflowReviewAccumulator;
use std::fs;
use std::path::Path;
use std::sync::Arc;

fn test_ctx(dir: &Path, mode: AgentMode) -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(crate::ui::NullInterface),
        file_cache: Arc::new(FileCache::new()),
        checkpoint_state: Arc::new(CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(FileTracker::new())),
        anchor_store: Arc::new(crate::tools::AnchorStore::new()),
        lua_tool_loader: None,
        mode,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(crate::config::Config::default()),
        run_policy: Default::default(),
        supporting_provenance: Vec::new(),
    }
}

fn run_git_output(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to execute: {e}", args));
    assert!(
        output.status.success(),
        "git {:?} in {} failed (exit {:?}):\nstdout: {}\nstderr: {}",
        args,
        dir.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {:?} failed to execute: {e}", args));
    assert!(
        output.status.success(),
        "git {:?} in {} failed (exit {:?}):\nstdout: {}\nstderr: {}",
        args,
        dir.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    run_git(dir.path(), &["init"]);
    run_git(dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(dir.path(), &["config", "user.name", "Test User"]);
    fs::write(dir.path().join("note.txt"), "hello\n").unwrap();
    run_git(dir.path(), &["add", "-A"]);
    run_git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

fn extract_text(result: &ToolOutput) -> String {
    result.text_content().unwrap_or_default().to_string()
}

#[test]
fn schema_exposes_readonly_worktree_inspection_and_uses_snake_case_fields() {
    let schema = GitTool.parameters();
    let properties = schema["properties"].as_object().unwrap();
    let actions = properties["action"]["enum"].as_array().unwrap();

    assert!(actions.iter().any(|value| value == "worktree_list"));
    assert!(!actions.iter().any(|value| value == "worktree_add"));
    assert!(!actions.iter().any(|value| value == "worktree_remove"));
    assert!(!properties.contains_key("worktree_path"));
    assert!(properties.contains_key("all_changes"));
    assert!(!properties.contains_key("all"));
    assert!(properties.contains_key("allow_empty"));
    assert!(!properties.contains_key("allowEmpty"));
    assert!(!properties.contains_key("worktreePath"));
    assert_eq!(properties["limit"]["type"], json!("integer"));
    assert_eq!(properties["limit"]["maximum"], json!(100));
}

#[tokio::test]
async fn git_status_reports_clean_repo() {
    let dir = setup_repo();
    let tool = GitTool;
    let result = tool
        .execute(
            "c1",
            json!({"action": "status"}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("state: clean"));
    assert_eq!(result.details["clean"], json!(true));
}

#[tokio::test]
async fn git_diff_ignores_empty_ref_fields() {
    let dir = setup_repo();
    let tool = GitTool;

    let result = tool
        .execute(
            "c-diff",
            json!({"action": "diff", "base": "", "head": ""}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(extract_text(&result), "No diff.");
    assert_eq!(result.details["base"], json!(null));
    assert_eq!(result.details["head"], json!(null));
}

#[tokio::test]
async fn git_stage_and_commit_work() {
    let dir = setup_repo();
    fs::write(dir.path().join("note.txt"), "hello world\n").unwrap();
    let tool = GitTool;

    let stage = tool
        .execute(
            "c-stage",
            json!({"action": "stage", "files": ["note.txt"]}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();
    assert!(!stage.is_error);

    let commit = tool
        .execute(
            "c-commit",
            json!({"action": "commit", "message": "update note"}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();
    assert!(!commit.is_error);
    assert!(extract_text(&commit).contains("update note"));

    let status = tool
        .execute(
            "c-status",
            json!({"action": "status"}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();
    assert!(!status.is_error);
    assert_eq!(status.details["clean"], json!(true));
}

#[tokio::test]
async fn git_stage_accepts_all_changes() {
    let dir = setup_repo();
    fs::write(dir.path().join("new.txt"), "new\n").unwrap();
    let tool = GitTool;

    let result = tool
        .execute(
            "c-stage-all",
            json!({"action": "stage", "all_changes": true}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.details["all_changes"], json!(true));
}

#[tokio::test]
async fn git_commit_accepts_allow_empty() {
    let dir = setup_repo();
    let tool = GitTool;

    let result = tool
        .execute(
            "c-empty-commit",
            json!({"action": "commit", "message": "empty commit", "allow_empty": true}),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.details["allow_empty"], json!(true));
    assert!(extract_text(&result).contains("empty commit"));
}

#[tokio::test]
async fn targeted_commit_preserves_existing_index_and_unrelated_worktree() {
    let dir = setup_repo();
    fs::write(dir.path().join("target.txt"), "target base\n").unwrap();
    fs::write(dir.path().join("staged.txt"), "staged base\n").unwrap();
    fs::write(dir.path().join("dirty.txt"), "dirty base\n").unwrap();
    run_git(dir.path(), &["add", "-A"]);
    run_git(dir.path(), &["commit", "-m", "add fixtures"]);

    fs::write(dir.path().join("target.txt"), "target changed\n").unwrap();
    fs::write(dir.path().join("staged.txt"), "staged changed\n").unwrap();
    fs::write(dir.path().join("dirty.txt"), "dirty changed\n").unwrap();
    run_git(dir.path(), &["add", "staged.txt"]);

    let tool = GitTool;
    let result = tool
        .execute(
            "c-targeted-commit",
            json!({
                "action": "commit",
                "message": "update target only",
                "files": ["target.txt"]
            }),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(!result.is_error, "{}", extract_text(&result));
    assert_eq!(result.details["preserve_index"], json!(true));
    assert!(extract_text(&result).contains("Included targeted path"));

    let committed_files = run_git_output(
        dir.path(),
        &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    );
    assert_eq!(committed_files, "target.txt");

    let status = run_git_output(dir.path(), &["status", "--porcelain=v1"]);
    assert!(
        status.lines().any(|line| line == "M  staged.txt"),
        "{status}"
    );
    assert!(
        !status.lines().any(|line| line.ends_with("target.txt")),
        "{status}"
    );
}

#[tokio::test]
async fn targeted_commit_rejects_noop_paths() {
    let dir = setup_repo();
    let tool = GitTool;

    let result = tool
        .execute(
            "c-targeted-noop",
            json!({
                "action": "commit",
                "message": "noop target",
                "files": ["note.txt"]
            }),
            test_ctx(dir.path(), AgentMode::Worker),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(extract_text(&result).contains("No changes to commit"));
    assert_eq!(
        run_git_output(dir.path(), &["rev-list", "--count", "HEAD"]),
        "1"
    );
}

#[tokio::test]
async fn git_restore_reverts_file_and_creates_checkpoint() {
    let dir = setup_repo();
    fs::write(dir.path().join("note.txt"), "changed\n").unwrap();
    let tool = GitTool;
    let ctx = test_ctx(dir.path(), AgentMode::Worker);
    let checkpoint_state = ctx.checkpoint_state.clone();

    let result = tool
        .execute(
            "c-restore",
            json!({"action": "restore", "files": ["note.txt"]}),
            ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "hello\n"
    );
    assert_eq!(checkpoint_state.checkpoints().len(), 1);
    assert!(result.details["checkpoint_id"].as_str().is_some());
}

#[tokio::test]
async fn planner_mode_blocks_mutating_git_actions() {
    let dir = setup_repo();
    let tool = GitTool;
    fs::write(dir.path().join("note.txt"), "changed\n").unwrap();

    let result = tool
        .execute(
            "c-stage",
            json!({"action": "stage", "files": ["note.txt"]}),
            test_ctx(dir.path(), AgentMode::Planner),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(extract_text(&result).contains("not permitted"));
}

#[tokio::test]
async fn planner_mode_allows_readonly_git_actions() {
    let dir = setup_repo();
    let tool = GitTool;

    let result = tool
        .execute(
            "c-status",
            json!({"action": "status"}),
            test_ctx(dir.path(), AgentMode::Planner),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(extract_text(&result).contains("repo:"));
}
