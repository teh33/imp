use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::config::AgentMode;
use crate::policy::RunPolicy;
use crate::tools::{AnchorStore, CheckpointState, FileCache, FileTracker, ToolContext};
use crate::workflow_review::TurnWorkflowReviewAccumulator;

const TOOLS: &str = r#"[{"name":"goto"},{"name":"interactiveElements"},{"name":"markdown"},{"name":"links"},{"name":"structuredData"},{"name":"detectForms"},{"name":"extract"},{"name":"click"},{"name":"fill"},{"name":"press"},{"name":"selectOption"},{"name":"setChecked"},{"name":"scroll"},{"name":"waitForSelector"},{"name":"getUrl"},{"name":"consoleLogs"}]"#;

fn context(cwd: &Path) -> ToolContext {
    let (update_tx, _) = tokio::sync::mpsc::channel(8);
    let (command_tx, _) = tokio::sync::mpsc::channel(8);
    ToolContext {
        cwd: cwd.to_path_buf(),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx,
        command_tx,
        ui: Arc::new(crate::ui::NullInterface),
        file_cache: Arc::new(FileCache::new()),
        checkpoint_state: Arc::new(CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(FileTracker::new())),
        anchor_store: Arc::new(AnchorStore::new()),
        lua_tool_loader: None,
        mode: AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(crate::config::Config::default()),
        run_policy: RunPolicy::default(),
        supporting_provenance: Vec::new(),
    }
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("{name}.sh"));
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn initialized(action: &str) -> String {
    format!(
        r#"while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2024-11-05"}}}}' ;;
*'"method":"notifications/initialized"'*) ;;
*'"method":"tools/list"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"tools":{TOOLS}}}}}' ;;
*'"method":"tools/call"'*) {action} ;;
esac
done"#
    )
}

fn config(binary: PathBuf) -> BrowserConfig {
    BrowserConfig {
        binary: Some(binary),
        timeout_ms: 5_000,
        max_response_bytes: 4096,
        ..Default::default()
    }
}

async fn start(tool: &BrowserTool, ctx: ToolContext) -> ToolOutput {
    tool.execute("start", json!({"action": "start"}), ctx)
        .await
        .unwrap()
}

fn session_id(output: &ToolOutput) -> &str {
    output.details["session_id"].as_str().unwrap()
}

fn text(output: &ToolOutput) -> &str {
    output
        .content
        .iter()
        .find_map(|block| match block {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

mod isolation;

#[tokio::test]
async fn startup_timeout_fails_without_creating_session() {
    let dir = TempDir::new().unwrap();
    let binary = script(dir.path(), "startup-timeout", "read line\nsleep 1");
    let mut browser_config = config(binary);
    browser_config.timeout_ms = 150;
    let tool = BrowserTool::new(browser_config);
    let output = start(&tool, context(dir.path())).await;
    assert!(output.is_error);
    assert!(text(&output).contains("initialize timed out"));
    assert_eq!(tool.sessions.lock().await.session_count_for_test(), 0);
}

#[tokio::test]
async fn malformed_and_mismatched_protocol_responses_fail_closed() {
    for (name, response, expected) in [
        ("malformed", "not-json", "invalid JSON"),
        (
            "mismatch",
            r#"{"jsonrpc":"2.0","id":99,"result":{}}"#,
            "mismatched response id",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let body = format!("read line\nprintf '%s\\n' '{response}'");
        let tool = BrowserTool::new(config(script(dir.path(), name, &body)));
        let output = start(&tool, context(dir.path())).await;
        assert!(output.is_error, "{name}");
        assert!(text(&output).contains(expected), "{}", text(&output));
    }
}

#[tokio::test]
async fn missing_required_tool_is_rejected() {
    let dir = TempDir::new().unwrap();
    let body = r#"while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}' ;;
*'"method":"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}' ;;
esac
done"#;
    let tool = BrowserTool::new(config(script(dir.path(), "missing-tool", body)));
    let output = start(&tool, context(dir.path())).await;
    assert!(output.is_error);
    assert!(text(&output).contains("required MCP tool `goto` is missing"));
}

#[tokio::test]
async fn oversized_and_partial_responses_are_bounded() {
    for (name, action, expected) in [
        (
            "oversized",
            "printf '%05000d\\n' 0",
            "response exceeded 4096 bytes",
        ),
        ("partial", "printf 'partial'; sleep 1", "timed out"),
    ] {
        let dir = TempDir::new().unwrap();
        let tool = BrowserTool::new(config(script(dir.path(), name, &initialized(action))));
        let ctx = context(dir.path());
        let started = start(&tool, ctx.clone()).await;
        assert!(!started.is_error, "{}", text(&started));
        let id = session_id(&started);
        let output = tool
            .execute(
                "call",
                json!({"action": "observe", "session_id": id, "timeout_ms": 150}),
                ctx,
            )
            .await
            .unwrap();
        assert!(output.is_error, "{name}");
        assert!(text(&output).contains(expected), "{}", text(&output));
    }
}

mod fixture;

#[tokio::test]
async fn private_network_flag_is_enabled_by_default_and_explicitly_optional() {
    for (blocked, expected) in [(true, "blocked"), (false, "allowed")] {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "if printf '%s' \"$*\" | grep -q -- '--block-private-networks'; then mode=blocked; else mode=allowed; fi\n\
             if [ \"$mode\" != \"{expected}\" ]; then exit 9; fi\n{}",
            initialized("printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}'")
        );
        let mut browser_config = config(script(dir.path(), expected, &body));
        browser_config.block_private_networks = blocked;
        let tool = BrowserTool::new(browser_config);
        let output = start(&tool, context(dir.path())).await;
        assert!(!output.is_error, "{expected}: {}", text(&output));
    }
}
