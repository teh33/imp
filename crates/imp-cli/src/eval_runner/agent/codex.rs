use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::super::process::ProcessOutcome;
use super::super::result::EvalAgentResult;
use super::super::spec::EvalTaskSpec;

pub(super) fn args(spec: &EvalTaskSpec, model: &str, thinking: &str) -> Vec<String> {
    vec![
        "exec".into(),
        "--ephemeral".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--json".into(),
        "--color".into(),
        "never".into(),
        "--sandbox".into(),
        "workspace-write".into(),
        "-c".into(),
        "approval_policy=\"never\"".into(),
        "-c".into(),
        format!("model_reasoning_effort=\"{}\"", reasoning(thinking)),
        "-m".into(),
        model.into(),
        spec.execution_prompt(),
    ]
}

pub(super) fn env(config_dir: &Path) -> Result<Vec<(String, String)>, std::io::Error> {
    fs::create_dir_all(config_dir)?;
    link_auth(config_dir)?;
    Ok(vec![(
        "CODEX_HOME".into(),
        config_dir.display().to_string(),
    )])
}

fn link_auth(config_dir: &Path) -> Result<(), std::io::Error> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(());
    };
    let source = Path::new(&home).join(".codex/auth.json");
    if !source.is_file() {
        return Ok(());
    }
    let destination = config_dir.join("auth.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(source, destination)?;
    #[cfg(not(unix))]
    fs::copy(source, destination).map(|_| ())?;
    Ok(())
}

pub(super) fn record_outcome(
    result: &mut EvalAgentResult,
    process: ProcessOutcome,
    stdout_path: &Path,
) -> bool {
    let content = fs::read_to_string(stdout_path).unwrap_or_default();
    let events = parse_events(&content);
    let fatal_error = events.iter().find_map(fatal_error);
    let warnings = events.iter().filter_map(item_error).collect::<Vec<_>>();
    let final_text = events.iter().rev().find_map(final_text);
    let usage = events
        .iter()
        .rev()
        .find(|event| event["type"] == "turn.completed")
        .and_then(|event| event.get("usage"))
        .map(normalize_usage)
        .unwrap_or_else(|| json!({}));
    let turns = events
        .iter()
        .filter(|event| event["type"] == "turn.started")
        .count();
    let tool_calls = events.iter().filter(|event| is_tool_item(event)).count();
    result.outcome = Some(json!({
        "status": if fatal_error.is_none() { "done" } else { "error" },
        "final_text": final_text,
        "error": fatal_error,
        "warnings": warnings,
        "metrics": { "turns": turns, "tool_calls": tool_calls },
        "usage": usage,
        "event_count": events.len(),
    }));
    process.success()
        && fatal_error.is_none()
        && events.iter().any(|event| event["type"] == "turn.completed")
}

fn parse_events(content: &str) -> Vec<Value> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn reasoning(thinking: &str) -> &str {
    match thinking {
        "off" | "none" => "none",
        "minimal" | "low" | "medium" | "high" | "xhigh" => thinking,
        _ => "high",
    }
}

fn fatal_error(event: &Value) -> Option<String> {
    match event["type"].as_str()? {
        "error" => event["message"].as_str().map(str::to_string),
        "turn.failed" => event["error"]["message"].as_str().map(str::to_string),
        _ => None,
    }
}

fn item_error(event: &Value) -> Option<String> {
    (event["type"] == "item.completed" && event["item"]["type"] == "error").then(|| {
        event["item"]["message"]
            .as_str()
            .unwrap_or("Codex warning")
            .to_string()
    })
}

fn final_text(event: &Value) -> Option<String> {
    (event["type"] == "item.completed" && event["item"]["type"] == "agent_message")
        .then(|| event["item"]["text"].as_str().map(str::to_string))?
}

fn is_tool_item(event: &Value) -> bool {
    event["type"] == "item.completed"
        && matches!(
            event["item"]["type"].as_str(),
            Some("command_execution" | "file_change" | "mcp_tool_call" | "web_search")
        )
}

fn normalize_usage(usage: &Value) -> Value {
    json!({
        "input_tokens": usage["input_tokens"].as_u64().unwrap_or(0),
        "output_tokens": usage["output_tokens"].as_u64().unwrap_or(0),
        "reasoning_output_tokens": usage["reasoning_output_tokens"].as_u64().unwrap_or(0),
        "cache_read_tokens": usage["cached_input_tokens"].as_u64().unwrap_or(0),
        "cache_write_tokens": 0,
        "input_includes_cache": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_turn_with_nonfatal_warning_succeeds() {
        let content = r#"{"type":"item.completed","item":{"type":"error","message":"warning"}}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":10}}"#;
        let events = parse_events(content);
        assert!(events.iter().find_map(fatal_error).is_none());
        assert_eq!(events.iter().filter_map(item_error).count(), 1);
        assert_eq!(
            normalize_usage(&events[2]["usage"])["cache_read_tokens"],
            60
        );
    }
}
