use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::tests::test_ctx;
use super::{ShellExecDef, ShellTool, ShellToolDef};
use crate::tools::{Tool, ToolOutput};

fn tool(command: &str, args: Vec<String>, timeout: u32) -> ShellTool {
    ShellTool::new(ShellToolDef {
        name: "runtime_test".into(),
        label: "Runtime Test".into(),
        description: "Exercises process runtime behavior".into(),
        readonly: true,
        params: HashMap::new(),
        exec: ShellExecDef {
            command: command.into(),
            args,
            timeout,
            truncate: "head".into(),
            install_hint: None,
        },
    })
}

fn text(output: &ToolOutput) -> &str {
    match &output.content[0] {
        imp_llm::ContentBlock::Text { text } => text,
        _ => panic!("expected text output"),
    }
}

#[tokio::test]
async fn shell_tool_closes_stdin_before_waiting() {
    let tool = tool(
        "sh",
        vec![
            "-c".into(),
            "if IFS= read -r line; then printf unexpected; else printf eof; fi".into(),
        ],
        1,
    );
    let temp_dir = tempfile::tempdir().unwrap();
    let result = tool
        .execute("stdin-eof", json!({}), test_ctx(temp_dir.path()))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(result.details["timed_out"], false);
    assert_eq!(text(&result), "eof");
}

#[tokio::test]
async fn shell_tool_cancellation_stops_running_process() {
    let tool = tool("sleep", vec!["60".into()], 30);
    let temp_dir = tempfile::tempdir().unwrap();
    let ctx = test_ctx(temp_dir.path());
    let cancelled = Arc::clone(&ctx.cancelled);
    let trigger = async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    };
    let (result, ()) = tokio::join!(tool.execute("cancel", json!({}), ctx), trigger);
    let result = result.unwrap();
    assert!(result.is_error);
    assert_eq!(result.details["cancelled"], true);
    assert_eq!(result.details["timed_out"], false);
}
