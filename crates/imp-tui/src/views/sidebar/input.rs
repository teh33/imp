use serde_json::Value;

use crate::views::tools::DisplayToolCall;

pub(super) fn tool_input_summary_rows(tc: &DisplayToolCall) -> Vec<String> {
    let Some(args) = tc.details.as_object() else {
        return value_to_summary_rows(&tc.details);
    };

    match tc.name.as_str() {
        "shell" | "bash" => summarize_named_fields(args, &["command", "workdir", "timeout"]),
        "read" => summarize_named_fields(args, &["path", "offset", "limit"]),
        "edit" => summarize_edit_fields(args),
        "write" => summarize_write_fields(args),
        "scan" => summarize_named_fields(args, &["action", "directory", "files", "task"]),
        "workflow" => summarize_named_fields(
            args,
            &[
                "action", "id", "title", "status", "priority", "parent", "deps", "verify", "notes",
                "reason", "run_id",
            ],
        ),
        "ask_user" => summarize_named_fields(
            args,
            &["question", "choices", "allow_other", "multi_select"],
        ),
        "web" => {
            summarize_named_fields(args, &["action", "query", "url", "provider", "maxResults"])
        }
        "work" => summarize_named_fields(
            args,
            &[
                "action",
                "kind",
                "id",
                "title",
                "status",
                "parent_work",
                "outcome",
                "summary",
            ],
        ),
        _ => summarize_object_fields(args),
    }
}

fn summarize_named_fields(args: &serde_json::Map<String, Value>, keys: &[&str]) -> Vec<String> {
    let mut rows = Vec::new();
    for key in keys {
        if let Some(value) = args.get(*key) {
            push_summary_row(&mut rows, key, value);
        }
    }
    rows
}

fn summarize_edit_fields(args: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut rows = summarize_named_fields(args, &["path"]);
    if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        rows.push(format!("edits: {}", edits.len()));
    } else {
        rows.extend(summarize_named_fields(
            args,
            &["oldText", "newText", "replaceAll"],
        ));
    }
    rows
}

fn summarize_write_fields(args: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut rows = summarize_named_fields(args, &["path"]);
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        rows.push(format!(
            "content: {} chars, {} lines",
            content.chars().count(),
            content.lines().count()
        ));
    }
    rows
}

fn summarize_object_fields(args: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut rows = Vec::new();
    for (key, value) in args {
        push_summary_row(&mut rows, key, value);
    }
    rows
}

fn value_to_summary_rows(value: &Value) -> Vec<String> {
    if value.is_null() {
        Vec::new()
    } else {
        vec![format!("value: {}", summarize_value(value))]
    }
}

fn push_summary_row(rows: &mut Vec<String>, key: &str, value: &Value) {
    if let Some(summary) = summarize_field_value(value) {
        rows.push(format!("{key}: {summary}"));
    }
}

fn summarize_field_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(summarize_text(text)),
        Value::Array(items) => Some(summarize_array(items)),
        Value::Object(obj) => Some(summarize_object(obj)),
        Value::Bool(_) | Value::Number(_) => Some(summarize_value(value)),
    }
}

fn summarize_value(value: &Value) -> String {
    match value {
        Value::String(text) => summarize_text(text),
        Value::Array(items) => summarize_array(items),
        Value::Object(obj) => summarize_object(obj),
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
    }
}

fn summarize_object(obj: &serde_json::Map<String, Value>) -> String {
    if obj.is_empty() {
        return "{}".to_string();
    }

    let mut fields = obj
        .iter()
        .filter_map(|(key, value)| {
            summarize_field_value(value).map(|summary| format!("{key}: {summary}"))
        })
        .collect::<Vec<_>>();
    fields.sort();
    format!("{{{}}}", fields.join(", "))
}

fn summarize_array(items: &[Value]) -> String {
    const MAX_ITEMS: usize = 6;
    let mut parts = items
        .iter()
        .take(MAX_ITEMS)
        .map(summarize_value)
        .collect::<Vec<_>>();
    if items.len() > MAX_ITEMS {
        parts.push(format!("… {} more", items.len() - MAX_ITEMS));
    }
    format!("[{}]", parts.join(", "))
}

fn summarize_text(text: &str) -> String {
    const MAX_TEXT_CHARS: usize = 240;
    const MAX_TEXT_LINES: usize = 4;

    let mut lines = text.lines().take(MAX_TEXT_LINES).collect::<Vec<_>>();
    let omitted_lines = text.lines().count().saturating_sub(lines.len());
    if lines.is_empty() && !text.is_empty() {
        lines.push(text);
    }

    let mut summary = lines.join("\\n");
    summary = truncated_scalar_preview(&summary, MAX_TEXT_CHARS);
    if omitted_lines > 0 {
        summary.push_str(&format!(" … {omitted_lines} more lines"));
    }
    summary
}

fn truncated_scalar_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}
