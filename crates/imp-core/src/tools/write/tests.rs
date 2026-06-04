use super::*;
use crate::tools::ToolContext;
use std::path::Path;
use std::sync::Arc;

fn test_ctx(dir: &Path) -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(crate::ui::NullInterface),
        file_cache: Arc::new(crate::tools::FileCache::new()),
        checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
        anchor_store: Arc::new(crate::tools::AnchorStore::new()),
        lua_tool_loader: None,
        mode: crate::config::AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(crate::config::Config::default()),
        run_policy: Default::default(),
        supporting_provenance: Vec::new(),
    }
}

fn test_ctx_with_policy(dir: &Path, overwrite_policy: WriteOverwritePolicy) -> ToolContext {
    let mut ctx = test_ctx(dir);
    let mut config = crate::config::Config::default();
    config.write.overwrite_policy = overwrite_policy;
    ctx.config = Arc::new(config);
    ctx
}

fn test_ctx_with_run_policy(dir: &Path, run_policy: crate::policy::RunPolicy) -> ToolContext {
    let mut ctx = test_ctx(dir);
    ctx.run_policy = run_policy;
    ctx
}

#[tokio::test]
async fn write_create_mode_refuses_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn old() {}\n").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c-create-mode",
            serde_json::json!({"path": "lib.rs", "content": "fn new() {}\n", "mode": "create"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result
        .text_content()
        .unwrap()
        .contains("refuses to overwrite"));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "fn old() {}\n");
}

#[tokio::test]
async fn write_overwrite_reports_symbol_diff() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn old() {}\n").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c-symbol-diff",
            serde_json::json!({"path": "lib.rs", "content": "fn new() {}\n", "mode": "overwrite"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.details["symbol_diff"]["added"][0], "new");
    assert_eq!(result.details["symbol_diff"]["removed"][0], "old");
}

#[tokio::test]
async fn write_validate_syntax_blocks_invalid_source() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn old() {}\n").unwrap();

    let tool = WriteTool;
    let result = tool
            .execute(
                "c-write-syntax",
                serde_json::json!({"path": "lib.rs", "content": "fn broken( {\n", "validateSyntax": true}),
                test_ctx(dir.path()),
            )
            .await
            .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().unwrap().contains("syntax errors"));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "fn old() {}\n");
}

#[tokio::test]
async fn write_path_policy_allows_matching_file() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c-allow-write",
            serde_json::json!({"path": "CHANGELOG.md", "content": "updated"}),
            test_ctx_with_run_policy(
                dir.path(),
                crate::policy::RunPolicy::new().allow_write("CHANGELOG.md"),
            ),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap(),
        "updated"
    );
}

#[tokio::test]
async fn write_path_policy_blocks_unlisted_file() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c-deny-write",
            serde_json::json!({"path": "src/lib.rs", "content": "updated"}),
            test_ctx_with_run_policy(
                dir.path(),
                crate::policy::RunPolicy::new().allow_write("CHANGELOG.md"),
            ),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().unwrap().contains("write allowlist"));
    assert!(!dir.path().join("src/lib.rs").exists());
}

#[tokio::test]
async fn write_path_policy_blocks_parent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let relative = pathdiff::diff_paths(outside.path().join("CHANGELOG.md"), dir.path()).unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c-traversal",
            serde_json::json!({"path": relative, "content": "updated"}),
            test_ctx_with_run_policy(
                dir.path(),
                crate::policy::RunPolicy::new().allow_write("CHANGELOG.md"),
            ),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result
        .text_content()
        .unwrap()
        .contains("outside the worker root"));
    assert!(!outside.path().join("CHANGELOG.md").exists());
}

#[tokio::test]
async fn write_path_policy_deny_overrides_allow() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c-deny-override",
            serde_json::json!({"path": "CHANGELOG.md", "content": "updated"}),
            test_ctx_with_run_policy(
                dir.path(),
                crate::policy::RunPolicy::new()
                    .allow_write("CHANGELOG.md")
                    .deny_write("CHANGELOG.md"),
            ),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().unwrap().contains("denylist"));
    assert!(!dir.path().join("CHANGELOG.md").exists());
}

#[tokio::test]
async fn write_path_policy_glob_allows_matching_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c-glob-write",
            serde_json::json!({"path": "docs/CHANGELOG.md", "content": "updated"}),
            test_ctx_with_run_policy(
                dir.path(),
                crate::policy::RunPolicy::new().allow_write("docs/*.md"),
            ),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("docs/CHANGELOG.md")).unwrap(),
        "updated"
    );
}

#[tokio::test]
async fn write_default_policy_warns_on_unread_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "original").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c-warn",
            serde_json::json!({"path": "existing.txt", "content": "updated"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.details["warning_codes"][0], "unread_overwrite");
    assert_eq!(result.details["overwritten"], true);
    assert!(result.details["checkpoint_id"].as_str().is_some());
}

#[tokio::test]
async fn write_require_read_policy_blocks_unread_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "original").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c-block-unread",
            serde_json::json!({"path": "existing.txt", "content": "updated"}),
            test_ctx_with_policy(dir.path(), WriteOverwritePolicy::RequireRead),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
}

#[tokio::test]
async fn write_block_stale_policy_blocks_stale_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "original").unwrap();

    let ctx = test_ctx_with_policy(dir.path(), WriteOverwritePolicy::BlockStale);
    ctx.file_tracker.lock().unwrap().record_read(&file);
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&file, "external").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c-block-stale",
            serde_json::json!({"path": "existing.txt", "content": "updated"}),
            ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "external");
}

#[tokio::test]
async fn write_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c1",
            serde_json::json!({"path": "new.txt", "content": "hello world"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let details = &result.details;
    assert_eq!(details["display_content"], "hello world");
    assert!(details["summary"]
        .as_str()
        .unwrap()
        .ends_with("new.txt: 11 bytes created"));
    let written = std::fs::read_to_string(dir.path().join("new.txt")).unwrap();
    assert_eq!(written, "hello world");
}

#[tokio::test]
async fn write_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c2",
            serde_json::json!({"path": "a/b/c/deep.txt", "content": "deep"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(dir.path().join("a/b/c/deep.txt")).unwrap();
    assert_eq!(written, "deep");
}

#[tokio::test]
async fn write_overwrite_creates_checkpoint_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("existing.txt");
    std::fs::write(&file, "original").unwrap();

    let tool = WriteTool;
    let ctx = test_ctx(dir.path());
    let checkpoint_state = ctx.checkpoint_state.clone();

    let result = tool
        .execute(
            "c-overwrite",
            serde_json::json!({"path": "existing.txt", "content": "updated"}),
            ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        checkpoint_state.original(&file).as_deref(),
        Some("original")
    );
    let checkpoints = checkpoint_state.checkpoints();
    assert_eq!(checkpoints.len(), 1);
    assert!(checkpoints[0].files.contains(&file));
}

#[tokio::test]
async fn write_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c4",
            serde_json::json!({"path": "empty.txt", "content": ""}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(dir.path().join("empty.txt")).unwrap();
    assert_eq!(written, "");
    assert_eq!(result.details["display_content"], "");
}

#[tokio::test]
async fn write_missing_path_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c5",
            serde_json::json!({"content": "hello"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
}

#[tokio::test]
async fn write_preserves_crlf_on_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crlf.txt");
    // Write a CRLF file first
    std::fs::write(&file, "line1\r\nline2\r\n").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c6",
            serde_json::json!({"path": "crlf.txt", "content": "new1\nnew2\n"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let raw = std::fs::read(dir.path().join("crlf.txt")).unwrap();
    // Should convert LF to CRLF since original had CRLF
    assert!(raw.windows(2).any(|w| w == b"\r\n"));
}

#[tokio::test]
async fn write_deep_nested_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
        .execute(
            "c7",
            serde_json::json!({"path": "x/y/z/w/v/deep.txt", "content": "deep content"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(dir.path().join("x/y/z/w/v/deep.txt")).unwrap();
    assert_eq!(written, "deep content");
}

#[tokio::test]
async fn write_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("exist.txt");
    std::fs::write(&file, "old content").unwrap();

    let tool = WriteTool;
    let result = tool
        .execute(
            "c3",
            serde_json::json!({"path": "exist.txt", "content": "new content"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("overwritten"));
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written, "new content");
}

#[tokio::test]
async fn write_includes_display_content_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;

    let result = tool
            .execute(
                "c8",
                serde_json::json!({"path": "preview.rs", "content": "fn main() {\n    println!(\"hi\");\n}\n"}),
                test_ctx(dir.path()),
            )
            .await
            .unwrap();

    assert!(!result.is_error);
    assert!(result.details["path"]
        .as_str()
        .unwrap()
        .ends_with("preview.rs"));
    assert!(result.details["summary"]
        .as_str()
        .unwrap()
        .ends_with("preview.rs: 34 bytes created"));
    assert_eq!(
        result.details["display_content"],
        "fn main() {\n    println!(\"hi\");\n}"
    );
    assert_eq!(result.details["display_note"], "");
}

#[tokio::test]
async fn write_display_content_truncates_large_content() {
    let dir = tempfile::tempdir().unwrap();
    let tool = WriteTool;
    let content = (0..100)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    let result = tool
        .execute(
            "c9",
            serde_json::json!({"path": "large.txt", "content": content}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let display_content = result.details["display_content"].as_str().unwrap();
    assert!(display_content.lines().count() <= 40);
    assert!(result.details["display_note"]
        .as_str()
        .unwrap()
        .contains("output truncated"));
}
