use super::*;
use crate::tools::ToolContext;
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

#[tokio::test]
async fn read_known_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c1", json!({"path": "hello.txt"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("line1"));
    assert!(text.contains("line3"));
}

#[tokio::test]
async fn read_start_end_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.txt");
    std::fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute(
            "c2",
            json!({"path": "data.txt", "start_line": 2, "end_line": 3}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("b"));
    assert!(text.contains("c"));
    assert!(!text.contains("a"));
    assert!(!text.contains("d"));
    assert_eq!(result.details["start_line"], 2);
    assert_eq!(result.details["end_line"], 3);
    assert_eq!(result.details["lines"], 2);
    assert_eq!(result.details["lines_read"], 2);
    assert_eq!(result.details["files"][0]["status"], "read");
    assert_eq!(result.details["files"][0]["lines_read"], 2);
}

#[tokio::test]
async fn read_file_not_found_suggestions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "hi").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c3", json!({"path": "helo.txt"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("File not found"));
    assert!(text.contains("hello.txt"));
}

#[tokio::test]
async fn read_binary_file_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.bin");
    std::fs::write(&file, b"\x00\x01\x02\x03").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c4", json!({"path": "data.bin"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(extract_text(&result).contains("Binary file"));
}

#[tokio::test]
async fn read_strips_at_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "content").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c5", json!({"path": "@test.txt"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(extract_text(&result).contains("content"));
}

#[tokio::test]
async fn read_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.txt");
    std::fs::write(&file, "").unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c6", json!({"path": "empty.txt"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
}

#[tokio::test]
async fn read_large_file_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.txt");
    let mut content = String::new();
    for i in 0..3000 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&file, &content).unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c7", json!({"path": "big.txt"}), test_ctx(dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("truncated"));
    // Should have the first lines
    assert!(text.contains("line 0"));
    // Details should indicate truncation
    assert_eq!(result.details["truncated"], true);
}

#[tokio::test]
async fn read_respects_configured_line_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("limited.txt");
    let mut content = String::new();
    for i in 0..800 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&file, &content).unwrap();

    let tool = ReadTool;
    let mut ctx = test_ctx(dir.path());
    ctx.read_max_lines = 500;
    let result = tool
        .execute("c7b", json!({"path": "limited.txt"}), ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("truncated"));
    assert!(text.contains("showing 500/800 lines"));
    assert_eq!(result.details["lines"], 500);
    assert_eq!(result.details["total_lines"], 800);
}

#[tokio::test]
async fn read_zero_line_limit_disables_line_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unlimited.txt");
    let mut content = String::new();
    for i in 0..800 {
        content.push_str(&format!("line {i}\n"));
    }
    std::fs::write(&file, &content).unwrap();

    let tool = ReadTool;
    let mut ctx = test_ctx(dir.path());
    ctx.read_max_lines = 0;
    let result = tool
        .execute("c7c", json!({"path": "unlimited.txt"}), ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(!text.contains("truncated"));
    assert!(text.contains("line 799"));
    assert_eq!(result.details["truncated"], false);
    assert_eq!(result.details["lines"], 800);
    assert_eq!(result.details["total_lines"], 800);
    assert!(result.details["path"]
        .as_str()
        .unwrap()
        .contains("unlimited.txt"));
}

#[tokio::test]
async fn read_directory_error() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();

    let tool = ReadTool;
    let result = tool
        .execute("c8", json!({"path": "subdir"}), test_ctx(dir.path()))
        .await;

    // Reading a directory should either error or produce an error output
    if let Ok(output) = result {
        assert!(output.is_error)
    }
}

#[tokio::test]
async fn read_can_emit_line_anchors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("anchored.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let tool = ReadTool;
    let ctx = test_ctx(dir.path());
    let result = tool
        .execute(
            "c-anchors",
            json!({"path": "anchored.txt", "start_line": 2, "end_line": 2, "anchors": true}),
            ctx.clone(),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("Anchors:"));
    let anchors = result.details["anchors"].as_array().unwrap();
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0]["line"], 2);
    let anchor = anchors[0]["anchor"].as_str().unwrap();
    let path = dir.path().join("anchored.txt");
    assert!(ctx.anchor_store.get(&path, anchor).is_some());
}

#[tokio::test]
async fn read_symbol_target_expands_rust_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("lib.rs"),
            "struct User;\n\nfn greet(name: &str) {\n    println!(\"hi {name}\");\n}\n\nfn other() {}\n",
        )
        .unwrap();

    let tool = ReadTool;
    let result = tool
        .execute(
            "c-symbol-rs",
            json!({"path": "lib.rs", "target": "lib.rs#greet"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("fn greet"));
    assert!(text.contains("println!"));
    assert!(!text.contains("fn other"));
    assert_eq!(result.details["start_line"], 3);
    assert_eq!(result.details["semantic_target"]["symbol"], "greet");
}

#[tokio::test]
async fn read_line_target_expands_typescript_enclosing_function() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
            dir.path().join("main.ts"),
            "export function greet(name: string) {\n  console.log(name);\n}\n\nexport function other() {}\n",
        )
        .unwrap();

    let tool = ReadTool;
    let result = tool
        .execute(
            "c-line-ts",
            json!({"path": "main.ts", "target": "main.ts:2"}),
            test_ctx(dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = extract_text(&result);
    assert!(text.contains("function greet"));
    assert!(text.contains("console.log"));
    assert!(!text.contains("function other"));
    assert_eq!(result.details["start_line"], 1);
    assert_eq!(result.details["semantic_target"]["symbol"], "greet");
}

fn extract_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
