use super::*;
use crate::ui::NullInterface;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(super) fn test_ctx(dir: &Path) -> ToolContext {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(NullInterface),
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

#[test]
fn load_shell_tools_registers_valid_defs_and_skips_invalid_ones() {
    let temp_dir = tempfile::tempdir().unwrap();
    let tools_dir = temp_dir.path().join("tools");
    std::fs::create_dir_all(tools_dir.join("nested")).unwrap();

    std::fs::write(
        tools_dir.join("nested").join("greet.toml"),
        r#"
name = "greet"
label = "Greet"
description = "Print a greeting"
readonly = true

[params.name]
type = "string"
description = "Name to greet"

[params.greeting]
type = "string"
description = "Greeting text"
optional = true

[exec]
command = "printf"
args = ["%s %s", "{greeting|hello}", "{name}"]
timeout = 5
truncate = "head"
"#,
    )
    .unwrap();

    std::fs::write(tools_dir.join("broken.toml"), "not = [valid").unwrap();

    let mut registry = ToolRegistry::new();
    load_shell_tools(&tools_dir, &mut registry).unwrap();

    let tool = registry.get("greet").expect("tool should be registered");
    assert_eq!(tool.name(), "greet");
    assert!(registry.get("broken").is_none());
}

#[tokio::test]
async fn shell_tool_executes_with_param_interpolation() {
    let tool = ShellTool::new(ShellToolDef {
        name: "greet".into(),
        label: "Greet".into(),
        description: "Print a greeting".into(),
        readonly: true,
        params: HashMap::from([
            (
                "name".into(),
                ShellParamDef {
                    param_type: "string".into(),
                    description: "Name to greet".into(),
                    optional: false,
                },
            ),
            (
                "greeting".into(),
                ShellParamDef {
                    param_type: "string".into(),
                    description: "Greeting text".into(),
                    optional: true,
                },
            ),
        ]),
        exec: ShellExecDef {
            command: "printf".into(),
            args: vec!["%s %s".into(), "{greeting|hello}".into(), "{name}".into()],
            timeout: 5,
            truncate: "head".into(),
            install_hint: None,
        },
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute(
            "call-1",
            json!({ "name": "Asher" }),
            test_ctx(temp_dir.path()),
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text output"),
    };
    assert_eq!(text, "hello Asher");
    assert_eq!(result.details["exit_code"], 0);
    assert_eq!(result.details["timed_out"], false);
}

#[tokio::test]
async fn shell_tool_default_param_used_when_not_provided() {
    let tool = ShellTool::new(ShellToolDef {
        name: "echo_default".into(),
        label: "Echo Default".into(),
        description: "Echo with default".into(),
        readonly: true,
        params: HashMap::from([(
            "msg".into(),
            ShellParamDef {
                param_type: "string".into(),
                description: "Message".into(),
                optional: true,
            },
        )]),
        exec: ShellExecDef {
            command: "echo".into(),
            args: vec!["{msg|default_value}".into()],
            timeout: 5,
            truncate: "head".into(),
            install_hint: None,
        },
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute("call-3", json!({}), test_ctx(temp_dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text output"),
    };
    assert!(text.contains("default_value"));
}

#[test]
fn shell_tool_required_param_missing_errors() {
    let defs = HashMap::from([(
        "name".into(),
        ShellParamDef {
            param_type: "string".into(),
            description: "Name".into(),
            optional: false,
        },
    )]);
    let provided = serde_json::Map::new();
    let result = validate_required_params(&defs, &provided);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("name"));
}

#[tokio::test]
async fn shell_tool_stderr_included_in_output() {
    let tool = ShellTool::new(ShellToolDef {
        name: "stderr_test".into(),
        label: "Stderr Test".into(),
        description: "Writes to stderr".into(),
        readonly: true,
        params: HashMap::new(),
        exec: ShellExecDef {
            command: "sh".into(),
            args: vec!["-c".into(), "echo stdout_msg; echo stderr_msg >&2".into()],
            timeout: 5,
            truncate: "head".into(),
            install_hint: None,
        },
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute("call-4", json!({}), test_ctx(temp_dir.path()))
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text output"),
    };
    assert!(text.contains("stdout_msg"));
    assert!(text.contains("stderr_msg"));
}

#[tokio::test]
async fn shell_tool_timeout() {
    let tool = ShellTool::new(ShellToolDef {
        name: "slow".into(),
        label: "Slow".into(),
        description: "Times out".into(),
        readonly: true,
        params: HashMap::new(),
        exec: ShellExecDef {
            command: "sleep".into(),
            args: vec!["60".into()],
            timeout: 1,
            truncate: "head".into(),
            install_hint: None,
        },
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute("call-5", json!({}), test_ctx(temp_dir.path()))
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(result.details["timed_out"], true);
}

#[tokio::test]
async fn shell_tool_reports_missing_commands_with_install_hint() {
    let tool = ShellTool::new(ShellToolDef {
        name: "missing".into(),
        label: "Missing".into(),
        description: "Missing command".into(),
        readonly: true,
        params: HashMap::new(),
        exec: ShellExecDef {
            command: "definitely-not-a-real-command".into(),
            args: Vec::new(),
            timeout: 5,
            truncate: "head".into(),
            install_hint: Some("brew install definitely-not-a-real-command".into()),
        },
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute("call-2", json!({}), test_ctx(temp_dir.path()))
        .await
        .unwrap();

    assert!(result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text output"),
    };
    assert!(text.contains("Command not found"));
    assert!(text.contains("Install hint"));
}
