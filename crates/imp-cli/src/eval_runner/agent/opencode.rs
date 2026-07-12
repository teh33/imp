use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::super::process::ProcessOutcome;
use super::super::result::EvalAgentResult;
use super::super::spec::EvalTaskSpec;

pub(super) fn args(
    spec: &EvalTaskSpec,
    provider: &str,
    model: &str,
    thinking: &str,
    cwd: &Path,
) -> Vec<String> {
    vec![
        "run".into(),
        "--pure".into(),
        "--format".into(),
        "json".into(),
        "--model".into(),
        qualified_model(provider, model),
        "--variant".into(),
        reasoning(thinking).into(),
        "--agent".into(),
        "build".into(),
        "--dir".into(),
        cwd.display().to_string(),
        spec.execution_prompt(),
    ]
}

pub(super) fn env(config_dir: &Path) -> Result<Vec<(String, String)>, std::io::Error> {
    fs::create_dir_all(config_dir)?;
    Ok(vec![
        (
            "OPENCODE_CONFIG_DIR".into(),
            config_dir.display().to_string(),
        ),
        ("OPENCODE_PURE".into(), "1".into()),
        ("OPENCODE_DISABLE_EXTERNAL_SKILLS".into(), "1".into()),
        ("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS".into(), "1".into()),
    ])
}

pub(super) fn record_outcome(
    result: &mut EvalAgentResult,
    process: ProcessOutcome,
    stdout_path: &Path,
) -> bool {
    let content = fs::read_to_string(stdout_path).unwrap_or_default();
    let events = parse_events(&content);
    let error = events.iter().find_map(event_error);
    let final_text = events.iter().rev().find_map(final_text);
    let finishes = events
        .iter()
        .filter(|event| event["type"] == "step_finish")
        .collect::<Vec<_>>();
    let usage = sum_usage(&finishes);
    let turns = events
        .iter()
        .filter(|event| event["type"] == "step_start")
        .count();
    let tool_calls = events
        .iter()
        .filter(|event| event["type"] == "tool_use")
        .count();
    result.outcome = Some(json!({
        "status": if error.is_none() { "done" } else { "error" },
        "final_text": final_text,
        "error": error,
        "metrics": { "turns": turns, "tool_calls": tool_calls },
        "usage": usage,
        "event_count": events.len(),
    }));
    process.success() && error.is_none() && !finishes.is_empty()
}

fn parse_events(content: &str) -> Vec<Value> {
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn qualified_model(provider: &str, model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    let provider = match provider {
        "openai-codex" => "openai",
        other => other,
    };
    format!("{provider}/{model}")
}

fn reasoning(thinking: &str) -> &str {
    match thinking {
        "off" | "none" => "none",
        "minimal" | "low" | "medium" | "high" | "xhigh" => thinking,
        _ => "high",
    }
}

fn event_error(event: &Value) -> Option<String> {
    (event["type"] == "error").then(|| {
        event["message"]
            .as_str()
            .or_else(|| event["error"]["message"].as_str())
            .unwrap_or("OpenCode agent error")
            .to_string()
    })
}

fn final_text(event: &Value) -> Option<String> {
    (event["type"] == "text")
        .then(|| {
            event["part"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
        })?
        .map(str::to_string)
}

fn sum_usage(finishes: &[&Value]) -> Value {
    let mut input = 0;
    let mut output = 0;
    let mut cache_read = 0;
    let mut cache_write = 0;
    let mut cost = 0.0;
    for event in finishes {
        let tokens = &event["part"]["tokens"];
        input += tokens["input"].as_u64().unwrap_or(0);
        output +=
            tokens["output"].as_u64().unwrap_or(0) + tokens["reasoning"].as_u64().unwrap_or(0);
        cache_read += tokens["cache"]["read"].as_u64().unwrap_or(0);
        cache_write += tokens["cache"]["write"].as_u64().unwrap_or(0);
        cost += event["part"]["cost"].as_f64().unwrap_or(0.0);
    }
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_read_tokens": cache_read,
        "cache_write_tokens": cache_write,
        "input_includes_cache": false,
        "cost_usd": cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_usage_across_tool_rounds() {
        let events = parse_events(
            r#"{"type":"step_finish","part":{"tokens":{"input":10,"output":2,"reasoning":3,"cache":{"read":20,"write":0}},"cost":0.1}}
{"type":"step_finish","part":{"tokens":{"input":4,"output":1,"reasoning":0,"cache":{"read":30,"write":2}},"cost":0.2}}"#,
        );
        let usage = sum_usage(&events.iter().collect::<Vec<_>>());
        assert_eq!(usage["input_tokens"], 14);
        assert_eq!(usage["output_tokens"], 6);
        assert_eq!(usage["cache_read_tokens"], 50);
        assert_eq!(usage["cache_write_tokens"], 2);
    }

    #[test]
    fn maps_codex_provider_to_opencode_model_name() {
        assert_eq!(
            qualified_model("openai-codex", "gpt-5.6-sol"),
            "openai/gpt-5.6-sol"
        );
    }
}
