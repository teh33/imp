use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::config::AgentMode;
use crate::policy::RunPolicy;
use crate::tools::{AnchorStore, CheckpointState, FileCache, FileTracker, ToolContext};
use crate::workflow_review::TurnWorkflowReviewAccumulator;

fn test_context(cwd: &Path) -> ToolContext {
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

fn fake_lightpanda(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("fake-lightpanda.sh");
    fs::write(
        &path,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}'
      ;;
    *'"method":"notifications/initialized"'*) ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"goto"},{"name":"interactiveElements"},{"name":"markdown"},{"name":"links"},{"name":"structuredData"},{"name":"detectForms"},{"name":"extract"},{"name":"click"},{"name":"fill"},{"name":"press"},{"name":"selectOption"},{"name":"setChecked"},{"name":"scroll"},{"name":"waitForSelector"},{"name":"getUrl"},{"name":"consoleLogs"}]}}'
      ;;
    *'"name":"goto"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"URL: https://example.com\nTitle: Example"}],"isError":false}}'
      ;;
    *'"name":"interactiveElements"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"button Example backendNodeId=7"}],"isError":false}}'
      ;;
  esac
done
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn output_text(output: &ToolOutput) -> &str {
    output
        .content
        .iter()
        .find_map(|block| match block {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

#[test]
fn browser_schema_exposes_semantic_actions_without_screenshot() {
    let schema = BrowserTool::new(BrowserConfig::default()).parameters();
    let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
    assert!(actions.contains(&json!("observe")));
    assert!(actions.contains(&json!("extract")));
    assert!(!actions.contains(&json!("screenshot")));
}

#[test]
fn action_policy_distinguishes_observation_from_input() {
    let tool = BrowserTool::new(BrowserConfig::default());
    let observe =
        json!({"action": "observe", "session_id": "browser_00000000000000000000000000000000"});
    let click = json!({"action": "click", "session_id": "browser_00000000000000000000000000000000", "selector": "button"});
    let submit = json!({"action": "click", "session_id": "browser_00000000000000000000000000000000", "selector": "button[type=submit]"});
    assert!(tool.is_readonly_call(&observe));
    assert!(!tool.is_readonly_call(&click));
    assert_eq!(
        tool.policy_metadata_for(&observe).action_kind,
        crate::reference_monitor::ToolActionKind::Read
    );
    assert!(tool.policy_metadata_for(&observe).network);
    assert!(!tool.policy_metadata_for(&click).requires_approval);
    assert!(tool.policy_metadata_for(&submit).requires_approval);
    let scroll = json!({"action": "scroll", "session_id": "browser_00000000000000000000000000000000", "y": 600});
    assert!(!tool.policy_metadata_for(&scroll).requires_approval);
}

#[tokio::test]
async fn fake_lightpanda_session_navigates_and_observes() {
    let dir = TempDir::new().unwrap();
    let config = BrowserConfig {
        binary: Some(fake_lightpanda(dir.path())),
        ..Default::default()
    };
    let tool = BrowserTool::new(config);
    let context = test_context(dir.path());

    let started = tool
        .execute("1", json!({"action": "start"}), context.clone())
        .await
        .unwrap();
    assert!(!started.is_error, "{}", output_text(&started));
    let session_id = started.details["session_id"].as_str().unwrap();

    let navigated = tool
        .execute(
            "2",
            json!({"action": "navigate", "session_id": session_id, "url": "https://example.com"}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(!navigated.is_error, "{}", output_text(&navigated));
    assert!(output_text(&navigated).contains("Example"));

    let observed = tool
        .execute(
            "3",
            json!({"action": "observe", "session_id": session_id}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(!observed.is_error, "{}", output_text(&observed));
    assert!(output_text(&observed).contains("backendNodeId=7"));

    let stopped = tool
        .execute(
            "4",
            json!({"action": "stop", "session_id": session_id}),
            context,
        )
        .await
        .unwrap();
    assert!(!stopped.is_error, "{}", output_text(&stopped));
}

#[test]
fn navigation_rejects_unsafe_url_forms() {
    for url in [
        "file:///tmp/secret",
        "javascript:alert(1)",
        "https://user:password@example.com/",
        "not a url",
    ] {
        let error = BrowserAction::Navigate
            .validate(&json!({"url": url}))
            .unwrap_err();
        assert!(!error.is_empty(), "{url}");
    }
    assert!(BrowserAction::Navigate
        .validate(&json!({"url": "https://example.com/path"}))
        .is_ok());
}

#[tokio::test]
async fn invalid_action_parameters_fail_before_starting_lightpanda() {
    let dir = TempDir::new().unwrap();
    let tool = BrowserTool::new(BrowserConfig {
        binary: Some(dir.path().join("missing-lightpanda")),
        ..Default::default()
    });
    let output = tool
        .execute(
            "1",
            json!({"action": "navigate", "session_id": "browser_00000000000000000000000000000000"}),
            test_context(dir.path()),
        )
        .await
        .unwrap();
    assert!(output.is_error);
    assert!(output_text(&output).contains("requires url"));
}

#[tokio::test]
#[ignore = "requires LIGHTPANDA_BIN and network access"]
async fn real_lightpanda_smoke_test() {
    let binary = std::env::var_os("LIGHTPANDA_BIN").expect("set LIGHTPANDA_BIN");
    let dir = TempDir::new().unwrap();
    let tool = BrowserTool::new(BrowserConfig {
        binary: Some(binary.into()),
        timeout_ms: 15_000,
        ..Default::default()
    });
    let context = test_context(dir.path());
    let started = tool
        .execute("1", json!({"action": "start"}), context.clone())
        .await
        .unwrap();
    assert!(!started.is_error, "{}", output_text(&started));
    let session_id = started.details["session_id"].as_str().unwrap();
    let navigated = tool
        .execute(
            "2",
            json!({"action": "navigate", "session_id": session_id, "url": "https://example.com"}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(!navigated.is_error, "{}", output_text(&navigated));
    let markdown = tool
        .execute(
            "3",
            json!({"action": "markdown", "session_id": session_id, "max_bytes": 4096}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(output_text(&markdown).contains("Example Domain"));
    let stopped = tool
        .execute(
            "4",
            json!({"action": "stop", "session_id": session_id}),
            context,
        )
        .await
        .unwrap();
    assert!(!stopped.is_error, "{}", output_text(&stopped));
}

#[tokio::test]
async fn missing_lightpanda_returns_actionable_error() {
    let dir = TempDir::new().unwrap();
    let tool = BrowserTool::new(BrowserConfig {
        binary: Some(dir.path().join("missing-lightpanda")),
        ..Default::default()
    });
    let output = tool
        .execute("1", json!({"action": "start"}), test_context(dir.path()))
        .await
        .unwrap();
    assert!(output.is_error);
    assert!(output_text(&output).contains("Install Lightpanda"));
}
