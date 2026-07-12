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
) -> Vec<String> {
    vec![
        "--provider".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        "--thinking".into(),
        thinking.into(),
        "--mode".into(),
        "json".into(),
        "--print".into(),
        "--no-session".into(),
        "--no-extensions".into(),
        "--no-skills".into(),
        "--no-prompt-templates".into(),
        "--tools".into(),
        "read,bash,edit,write".into(),
        spec.execution_prompt(),
    ]
}

pub(super) fn record_outcome(
    result: &mut EvalAgentResult,
    process: ProcessOutcome,
    stdout_path: &Path,
) -> bool {
    let content = fs::read_to_string(stdout_path).unwrap_or_default();
    let events = content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let error = events.iter().find_map(event_error);
    let final_text = events.iter().rev().find_map(final_text);
    let tool_calls = count_events(&events, "tool_execution_start");
    let turns = count_events(&events, "turn_start");
    let usage = events
        .iter()
        .rev()
        .find(|event| event["type"] == "agent_end")
        .and_then(|event| event["messages"].as_array())
        .map(|messages| sum_usage(messages))
        .unwrap_or_else(|| json!({}));
    result.outcome = Some(json!({
        "status": if error.is_none() { "done" } else { "error" },
        "final_text": final_text,
        "error": error,
        "metrics": { "turns": turns, "tool_calls": tool_calls },
        "usage": usage,
        "event_count": events.len(),
    }));
    process.success() && error.is_none() && events.iter().any(|event| event["type"] == "agent_end")
}

fn count_events(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["type"] == kind).count()
}

fn event_error(event: &Value) -> Option<String> {
    let message = event.get("message")?;
    (message["role"] == "assistant" && message["stopReason"] == "error").then(|| {
        message["errorMessage"]
            .as_str()
            .unwrap_or("Pi agent error")
            .to_string()
    })
}

fn final_text(event: &Value) -> Option<String> {
    let message = event.get("message")?;
    if message["role"] != "assistant" {
        return None;
    }
    let text = message["content"]
        .as_array()?
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn sum_usage(messages: &[Value]) -> Value {
    let (mut input, mut output, mut cache_read, mut cache_write) = (0, 0, 0, 0);
    let mut cost = 0.0;
    for usage in messages.iter().filter_map(|message| message.get("usage")) {
        input += usage["input"].as_u64().unwrap_or(0);
        output += usage["output"].as_u64().unwrap_or(0);
        cache_read += usage["cacheRead"].as_u64().unwrap_or(0);
        cache_write += usage["cacheWrite"].as_u64().unwrap_or(0);
        cost += usage["cost"]["total"].as_f64().unwrap_or(0.0);
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
    fn detects_assistant_error_despite_zero_process_exit() {
        let event = json!({
            "message": {
                "role": "assistant",
                "stopReason": "error",
                "errorMessage": "no auth"
            }
        });
        assert_eq!(event_error(&event).as_deref(), Some("no auth"));
    }
}
