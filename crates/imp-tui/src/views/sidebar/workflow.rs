use serde_json::Value;

use crate::views::tools::DisplayToolCall;

pub(super) fn format_workflow_output(tc: &DisplayToolCall) -> Vec<String> {
    let mut lines = Vec::new();
    let action = tc
        .details
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !action.is_empty() {
        lines.push("request".to_string());
        lines.push(format!("  action {action}"));

        match action {
            "create" => push_workflow_request_fields(
                &mut lines,
                tc,
                &[
                    "title",
                    "description",
                    "verify",
                    "priority",
                    "parent",
                    "deps",
                    "labels",
                ],
            ),
            "update" => push_workflow_request_fields(
                &mut lines,
                tc,
                &["id", "status", "title", "description", "priority", "notes"],
            ),
            "run" => push_workflow_request_fields(
                &mut lines,
                tc,
                &[
                    "id",
                    "run_id",
                    "scope",
                    "target",
                    "jobs",
                    "background",
                    "dry_run",
                    "review",
                    "timeout",
                    "idle_timeout",
                    "runtime",
                ],
            ),
            "close" | "reopen" | "fail" => {
                push_workflow_request_fields(&mut lines, tc, &["id", "reason", "unit"])
            }
            "notes_append" | "decision_add" | "decision_resolve" => push_workflow_request_fields(
                &mut lines,
                tc,
                &["id", "notes", "description", "resolve_decisions", "unit"],
            ),
            "dep_add" | "dep_remove" => {
                push_workflow_request_fields(&mut lines, tc, &["from_id", "dep_id"])
            }
            "delete" => push_workflow_request_fields(&mut lines, tc, &["id", "title"]),
            "fact_create" => push_workflow_request_fields(&mut lines, tc, &["unit_id", "unit"]),
            _ => push_workflow_request_fields(
                &mut lines,
                tc,
                &["id", "run_id", "reason", "by", "status", "count"],
            ),
        }
    }

    if has_live_workflow_output(tc) {
        push_blank_if_needed(&mut lines);
        lines.push("live output".to_string());
        if !tc.streaming_output.is_empty() {
            lines.extend(tc.streaming_output.lines().map(|line| format!("  {line}")));
        } else {
            lines.extend(tc.streaming_lines.iter().map(|line| format!("  {line}")));
        }
    }

    if let Some(view) = tc.details.get("view") {
        if let Some(summary) = view.get("summary") {
            push_blank_if_needed(&mut lines);
            lines.push("summary".to_string());
            lines.push(format!("  {}", format_workflow_summary(summary)));
        }

        if let Some(units) = view.get("units").and_then(Value::as_array) {
            if !units.is_empty() {
                push_blank_if_needed(&mut lines);
                lines.push("units".to_string());
            }
            for unit in units {
                push_workflow_unit_lines(&mut lines, unit);
            }
        }
    } else if !tc.streaming_output.is_empty() {
        lines.extend(tc.streaming_output.lines().map(String::from));
    } else if !tc.streaming_lines.is_empty() {
        lines.extend(tc.streaming_lines.clone());
    } else if let Some(ref output) = tc.output {
        lines.extend(output.lines().map(String::from));
    }

    if lines.is_empty() {
        vec!["Running…".to_string()]
    } else {
        lines
    }
}

fn has_live_workflow_output(tc: &DisplayToolCall) -> bool {
    tc.output.is_none() && (!tc.streaming_output.is_empty() || !tc.streaming_lines.is_empty())
}

fn push_blank_if_needed(lines: &mut Vec<String>) {
    if !lines.is_empty() && lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
}

fn push_workflow_request_fields(lines: &mut Vec<String>, tc: &DisplayToolCall, keys: &[&str]) {
    for key in keys {
        push_workflow_detail_line(lines, key, tc.details.get(*key));
    }
}

fn format_workflow_summary(summary: &Value) -> String {
    let total = summary
        .get("total_units")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let closed = summary
        .get("total_closed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let failed = summary
        .get("total_failed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let awaiting = summary
        .get("total_awaiting_verify")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let skipped = summary
        .get("total_skipped")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut parts = vec![format!("{total} units")];
    if closed > 0 {
        parts.push(format!("{closed} done"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if awaiting > 0 {
        parts.push(format!("{awaiting} verify"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    parts.join(" · ")
}

fn push_workflow_unit_lines(lines: &mut Vec<String>, unit: &Value) {
    let status = unit
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("queued");
    let marker = match status {
        "running" => "▶",
        "done" => "✓",
        "failed" => "✗",
        "blocked" => "!",
        _ => "…",
    };
    let id = unit.get("id").and_then(Value::as_str).unwrap_or("?");
    let title = unit.get("title").and_then(Value::as_str).unwrap_or("");
    lines.push(format!("  {marker} {id} · {title}"));

    let mut meta = Vec::new();
    meta.push(status.to_string());
    if let Some(round) = unit.get("round").and_then(Value::as_u64) {
        meta.push(format!("wave {round}"));
    }
    if let Some(agent) = unit.get("agent").and_then(Value::as_str) {
        meta.push(agent.to_string());
    }
    if let Some(duration) = unit.get("duration_secs").and_then(Value::as_u64) {
        meta.push(format!("{duration}s"));
    }
    if !meta.is_empty() {
        lines.push(format!("    {}", meta.join(" · ")));
    }
    if let Some(error) = unit.get("error").and_then(Value::as_str) {
        lines.push(format!("    error: {error}"));
    }
}

fn push_workflow_detail_line(lines: &mut Vec<String>, key: &str, value: Option<&Value>) {
    let Some(value) = value else {
        return;
    };
    let rendered = match value {
        Value::Null => return,
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Bool(b) => Some(b.to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            if let (Some(kind), Some(ids)) = (
                map.get("kind").and_then(Value::as_str),
                map.get("ids").and_then(Value::as_array),
            ) {
                let ids = ids
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{kind}: {ids}")
            } else if let (Some(kind), Some(id)) = (
                map.get("kind").and_then(Value::as_str),
                map.get("id").and_then(Value::as_str),
            ) {
                format!("{kind}: {id}")
            } else if let (Some(agent), Some(model)) = (
                map.get("direct_agent").and_then(Value::as_str),
                map.get("model").and_then(Value::as_str),
            ) {
                format!("{agent} · {model}")
            } else if let (Some(id), Some(title)) = (
                map.get("id").and_then(Value::as_str),
                map.get("title").and_then(Value::as_str),
            ) {
                let status = map
                    .get("status")
                    .and_then(Value::as_str)
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default();
                format!("{id} · {title}{status}")
            } else {
                serde_json::to_string(value).unwrap_or_default()
            }
        }
    };
    if !rendered.is_empty() {
        lines.push(format!("  {key} {rendered}"));
    }
}
