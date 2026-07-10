use super::*;
use crate::tools::ToolContext;
use std::sync::Arc;

fn test_ctx(dir: &std::path::Path) -> ToolContext {
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

#[tokio::test]
async fn edit_target_guard_requires_old_text_inside_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(
        &file,
        "fn target() {\n    let a = 1;\n}\n\nfn other() {\n    let b = 2;\n}\n",
    )
    .unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-target",
            json!({
                "path": "lib.rs",
                "target": "target",
                "oldText": "let b = 2;",
                "newText": "let b = 3;"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result
        .text_content()
        .unwrap()
        .contains("inside target symbol"));
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("let b = 2;"));
}

#[tokio::test]
async fn edit_validate_syntax_blocks_invalid_rust() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "fn target() {\n    let a = 1;\n}\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-syntax",
            json!({
                "path": "lib.rs",
                "target": "target",
                "oldText": "let a = 1;",
                "newText": "let a = ;",
                "validateSyntax": true
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().unwrap().contains("syntax errors"));
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("let a = 1;"));
}

#[tokio::test]
async fn edit_target_guard_and_syntax_validation_succeed_for_typescript() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.ts");
    std::fs::write(&file, "export function target() {\n  return 1;\n}\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-ts",
            json!({
                "path": "main.ts",
                "target": "target",
                "oldText": "return 1;",
                "newText": "return 2;",
                "validateSyntax": true
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("return 2;"));
    assert_eq!(result.details["syntax_validation"]["valid"], true);
}

#[tokio::test]
async fn edit_exact_match() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c1",
            json!({
                "path": "test.rs",
                "oldText": "println!(\"hello\")",
                "newText": "println!(\"world\")"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("world"));
    assert!(!written.contains("hello"));
}

#[tokio::test]
async fn edit_dry_run_returns_diff_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dry.txt");
    std::fs::write(&file, "alpha\n").unwrap();

    let tool = EditTool;
    let ctx = test_ctx(dir.path());
    let checkpoint_state = ctx.checkpoint_state.clone();
    let result = tool
        .execute(
            "c-dry",
            json!({
                "path": "dry.txt",
                "oldText": "alpha",
                "newText": "beta",
                "dryRun": true
            }),
            ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\n");
    assert!(checkpoint_state.checkpoints().is_empty());
    assert_eq!(result.details["dry_run"], true);
    let text = result.text_content().unwrap();
    assert!(text.contains("beta"));
    assert!(text.contains("dry run"));
}

#[tokio::test]
async fn edit_expected_occurrences_mismatch_does_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("expected-mismatch.txt");
    std::fs::write(&file, "foo foo\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-expected-mismatch",
            json!({
                "path": "expected-mismatch.txt",
                "oldText": "foo",
                "newText": "bar",
                "expectedOccurrences": 1
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "foo foo\n");
    assert!(result.text_content().unwrap().contains("found 2"));
}

#[tokio::test]
async fn edit_expected_occurrences_success_writes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("expected-success.txt");
    std::fs::write(&file, "foo\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-expected-success",
            json!({
                "path": "expected-success.txt",
                "oldText": "foo",
                "newText": "bar",
                "expectedOccurrences": 1
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "bar\n");
    assert_eq!(result.details["exact_occurrences"], 1);
    assert_eq!(result.details["replacements"], 1);
}

#[tokio::test]
async fn edit_replace_all_replaces_exact_matches() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("replace-all.txt");
    std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c-replace-all",
            json!({
                "path": "replace-all.txt",
                "oldText": "foo",
                "newText": "zap",
                "replaceAll": true,
                "expectedOccurrences": 3
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "zap bar zap baz zap\n"
    );
    assert_eq!(result.details["replace_all"], true);
    assert_eq!(result.details["replacements"], 3);
}

#[tokio::test]
async fn edit_creates_checkpoint_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("checkpoint.txt");
    std::fs::write(&file, "alpha\n").unwrap();

    let tool = EditTool;
    let ctx = test_ctx(dir.path());
    let checkpoint_state = ctx.checkpoint_state.clone();

    let result = tool
        .execute(
            "c-checkpoint",
            json!({
                "path": "checkpoint.txt",
                "oldText": "alpha",
                "newText": "beta"
            }),
            ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(checkpoint_state.original(&file).as_deref(), Some("alpha\n"));
    assert_eq!(checkpoint_state.checkpoints().len(), 1);
}

#[tokio::test]
async fn edit_fuzzy_trailing_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ws.txt");
    // File has trailing spaces on lines
    std::fs::write(&file, "hello   \nworld   \n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c2",
            json!({
                "path": "ws.txt",
                "oldText": "hello\nworld",
                "newText": "goodbye\nuniverse"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error, "Expected success but got error");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("goodbye"));
}

#[tokio::test]
async fn edit_fuzzy_unicode_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("uni.txt");
    // File has smart quotes
    std::fs::write(&file, "he said \u{201C}hello\u{201D}\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c3",
            json!({
                "path": "uni.txt",
                "oldText": "he said \"hello\"",
                "newText": "she said \"bye\""
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error, "Expected success but got error");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("bye"));
}

#[tokio::test]
async fn edit_crlf_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("crlf.txt");
    std::fs::write(&file, "line1\r\nline2\r\nline3\r\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c5",
            json!({
                "path": "crlf.txt",
                "oldText": "line2",
                "newText": "replaced"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("replaced"));
    // CRLF line endings should be preserved
    assert!(written.contains("\r\n"));
    assert!(!written.contains("line2"));
}

#[tokio::test]
async fn edit_replaces_first_occurrence_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("multi.txt");
    std::fs::write(&file, "foo bar foo baz foo\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c6",
            json!({
                "path": "multi.txt",
                "oldText": "foo",
                "newText": "REPLACED"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(&file).unwrap();
    // Should replace only the first occurrence
    assert_eq!(written, "REPLACED bar foo baz foo\n");
}

#[tokio::test]
async fn edit_ignores_empty_transaction_array_with_single_edit_fields() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("single.txt");
    std::fs::write(&file, "old value\n").unwrap();

    let result = EditTool
        .execute(
            "empty-edits",
            json!({
                "path": "single.txt",
                "old_text": "old value",
                "new_text": "new value",
                "edits": []
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "new value\n");
}

#[tokio::test]
async fn edit_empty_transaction_without_single_fields_keeps_transaction_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = EditTool
        .execute("empty-edits", json!({"edits": []}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|block| match block {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("Missing or empty edits array"));
}

#[tokio::test]
async fn edit_ignores_empty_anchor_with_exact_edit_fields() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("single.txt");
    std::fs::write(&file, "old value\n").unwrap();

    let result = EditTool
        .execute(
            "empty-anchor",
            json!({
                "path": "single.txt",
                "old_text": "old value",
                "new_text": "new value",
                "anchor_start": "",
                "anchor_end": ""
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "new value\n");
}

#[tokio::test]
async fn edit_empty_old_text_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.txt");
    std::fs::write(&file, "some content\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c7",
            json!({
                "path": "empty.txt",
                "oldText": "",
                "newText": "replacement"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("old_text"));
}

#[tokio::test]
async fn edit_nonexistent_file_error() {
    let dir = tempfile::tempdir().unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c8",
            json!({
                "path": "does_not_exist.txt",
                "oldText": "hello",
                "newText": "world"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("File not found"));
}

#[tokio::test]
async fn edit_missing_path_error() {
    let dir = tempfile::tempdir().unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c9",
            json!({
                "oldText": "hello",
                "newText": "world"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("path"));
}

#[tokio::test]
async fn edit_warns_on_unread_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unread.txt");
    std::fs::write(&file, "original content here\n").unwrap();

    // Use a fresh tracker (file never read)
    let tool = EditTool;
    let result = tool
        .execute(
            "c10",
            json!({
                "path": "unread.txt",
                "oldText": "original content",
                "newText": "changed content"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(
        !result.is_error,
        "edit should succeed even without prior read"
    );
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        text.contains("Warning"),
        "expected unread-file warning in output, got: {text}"
    );
}

#[tokio::test]
async fn anchored_edit_replaces_validated_range_and_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("anchored.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    let ctx = test_ctx(dir.path());
    let lines = ["beta"];
    let anchors = ctx.anchor_store.record_lines(
        &file,
        super::super::stable_hash("alpha\nbeta\ngamma\n"),
        2,
        &lines,
    );

    let result = EditTool
        .execute(
            "c-anchor",
            json!({
                "path": "anchored.txt",
                "anchor_start": anchors[0].id,
                "new_text": "BETA",
            }),
            ctx.clone(),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "alpha\nBETA\ngamma\n"
    );
    assert_eq!(
        ctx.checkpoint_state.original(&file).as_deref(),
        Some("alpha\nbeta\ngamma\n")
    );
    assert_eq!(result.details["anchored"], true);
    assert_eq!(result.details["action"], "edit");
    assert_eq!(result.details["mode"], "anchored");
    assert_eq!(result.details["start_line"], 2);
    assert_eq!(result.details["end_line"], 2);
    assert!(
        result.details["refreshed_anchors"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
}

#[tokio::test]
async fn anchored_edit_rejects_stale_anchor_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stale.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    let ctx = test_ctx(dir.path());
    let lines = ["beta"];
    let anchors = ctx.anchor_store.record_lines(
        &file,
        super::super::stable_hash("alpha\nbeta\ngamma\n"),
        2,
        &lines,
    );
    std::fs::write(&file, "alpha\nchanged\ngamma\n").unwrap();

    let result = EditTool
        .execute(
            "c-anchor-stale",
            json!({
                "path": "stale.txt",
                "anchor_start": anchors[0].id,
                "new_text": "BETA",
            }),
            ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().unwrap().contains("Stale anchor"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "alpha\nchanged\ngamma\n"
    );
}

#[tokio::test]
async fn anchored_edit_dry_run_does_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dry-anchor.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let ctx = test_ctx(dir.path());
    let lines = ["beta"];
    let anchors =
        ctx.anchor_store
            .record_lines(&file, super::super::stable_hash("alpha\nbeta\n"), 2, &lines);

    let result = EditTool
        .execute(
            "c-anchor-dry",
            json!({
                "path": "dry-anchor.txt",
                "anchor_start": anchors[0].id,
                "new_text": "BETA",
                "dry_run": true,
            }),
            ctx.clone(),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
    assert!(ctx.checkpoint_state.checkpoints().is_empty());
    assert!(result.text_content().unwrap().contains("dry run"));
}

#[tokio::test]
async fn edit_with_edits_uses_transaction_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("transaction.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();

    let result = EditTool
        .execute(
            "c-transaction",
            json!({
                "path": "transaction.txt",
                "edits": [
                    {"oldText": "alpha", "newText": "ALPHA"},
                    {"oldText": "beta", "newText": "BETA"}
                ]
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ALPHA\nBETA\n");
    assert_eq!(result.details["transaction"], true);
    assert_eq!(result.details["edit_count"], 2);
}

#[tokio::test]
async fn edit_no_match_error() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nope.txt");
    std::fs::write(&file, "some content here\n").unwrap();

    let tool = EditTool;
    let result = tool
        .execute(
            "c4",
            json!({
                "path": "nope.txt",
                "oldText": "this text does not exist",
                "newText": "replacement"
            }),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(result.is_error);
    let text = result
        .content
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(text.contains("Could not find"));
}
