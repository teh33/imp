use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::theme::Theme;
use crate::views::tools::DisplayToolCall;

use super::tool_output_text;

pub(super) fn styled_work_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc
        .details
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("work");
    let mut lines = vec![Line::from(vec![
        Span::styled("▣", theme.accent_style()),
        Span::styled("Work", theme.header_style()),
        Span::styled(format!(" · {action}"), theme.muted_style()),
    ])];

    if let Some(kind) = tc.details.get("kind").and_then(Value::as_str) {
        lines.push(work_kv_line("kind", kind, theme));
    }
    if let Some(status) = tc.details.get("status").and_then(Value::as_str) {
        lines.push(work_kv_line("status", status, theme));
    }
    if let Some(path) = tc.details.get("path").and_then(Value::as_str) {
        lines.push(work_kv_line("path", path, theme));
    }

    if let Some(item) = tc.details.get("item") {
        push_work_item(&mut lines, item, theme);
    }
    if let Some(items) = tc.details.get("items").and_then(Value::as_array) {
        push_blank_line(&mut lines);
        lines.push(Line::from(Span::styled(
            format!(
                "{} item{}",
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            ),
            theme.header_style(),
        )));
        for item in items {
            push_work_item(&mut lines, item, theme);
        }
    }
    if let Some(policy) = tc.details.get("policy") {
        push_work_policy_summary(&mut lines, policy, theme);
    }
    if let Some(provenance) = tc.details.get("provenance") {
        push_work_provenance_summary(&mut lines, provenance, theme);
    }

    if lines.len() == 1 {
        if let Some(output) = tool_output_text(tc) {
            lines.extend(
                output
                    .lines()
                    .map(|line| Line::from(Span::styled(line.to_string(), theme.muted_style()))),
            );
        } else {
            lines.push(Line::from(Span::styled("Running…", theme.muted_style())));
        }
    }

    lines
}

fn push_work_item(lines: &mut Vec<Line<'static>>, item: &Value, theme: &Theme) {
    let Some(obj) = item.as_object() else {
        lines.push(Line::from(Span::styled(
            format_work_value(item),
            theme.muted_style(),
        )));
        return;
    };

    push_blank_line(lines);
    let id = obj.get("id").and_then(Value::as_str).unwrap_or("work item");
    let title = obj
        .get("title")
        .or_else(|| obj.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let status = obj.get("status").and_then(Value::as_str);
    lines.push(Line::from(vec![
        Span::styled("● ", style_for_work_status(status, theme)),
        Span::styled(
            id.to_string(),
            theme.accent_style().add_modifier(Modifier::BOLD),
        ),
        if title.is_empty() {
            Span::raw(String::new())
        } else {
            Span::styled(format!("  {title}"), Style::default().fg(theme.fg))
        },
    ]));
    if let Some(status) = status {
        lines.push(work_kv_line("status", status, theme));
    }
    for key in ["parent", "parent_work", "context_pack", "stream_id"] {
        if let Some(value) = obj.get(key) {
            push_work_value_line(lines, key, value, theme);
        }
    }
    for key in [
        "acceptance",
        "checks",
        "depends_on",
        "links",
        "source_refs",
        "evidence_required",
        "topics",
    ] {
        if let Some(value) = obj.get(key) {
            push_work_value_line(lines, key, value, theme);
        }
    }
}

fn push_work_policy_summary(lines: &mut Vec<Line<'static>>, policy: &Value, theme: &Theme) {
    let Some(map) = policy.as_object() else {
        return;
    };

    push_blank_line(lines);
    let decision = map
        .get("decision")
        .and_then(|value| value.get("decision"))
        .and_then(Value::as_str)
        .unwrap_or("checked");
    let tool = map
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("work");
    let mode = map
        .get("autonomy_mode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    lines.push(Line::from(vec![
        Span::styled("policy ", theme.header_style()),
        Span::styled(
            decision.to_string(),
            style_for_policy_decision(decision, theme),
        ),
        Span::styled(format!(" · {tool} · {mode}"), theme.muted_style()),
    ]));

    if let Some(scope) = map.get("resource_scope") {
        let formatted = format_work_value(scope);
        if !formatted.is_empty() {
            lines.push(work_kv_line("scope", &formatted, theme));
        }
    }
    if let Some(labels) = map.get("trust_labels") {
        let formatted = format_work_value(labels);
        if !formatted.is_empty() {
            lines.push(work_kv_line("trust", &formatted, theme));
        }
    }
}

fn push_work_provenance_summary(lines: &mut Vec<Line<'static>>, provenance: &Value, theme: &Theme) {
    let Some(map) = provenance.as_object() else {
        return;
    };
    let trust = map.get("trust").and_then(Value::as_str).unwrap_or("");
    let risk = map.get("risk").map(format_work_value).unwrap_or_default();
    if trust.is_empty() && risk.is_empty() {
        return;
    }

    push_blank_line(lines);
    lines.push(Line::from(vec![
        Span::styled("provenance ", theme.header_style()),
        Span::styled(trust.to_string(), theme.muted_style()),
        if risk.is_empty() {
            Span::raw(String::new())
        } else {
            Span::styled(format!(" · {risk}"), theme.muted_style())
        },
    ]));
}

fn style_for_policy_decision(decision: &str, theme: &Theme) -> Style {
    match decision {
        "allow" | "allowed" => theme.success_style(),
        "deny" | "denied" | "blocked" => theme.error_style(),
        _ => theme.warning_style(),
    }
}

fn push_work_value_line(lines: &mut Vec<Line<'static>>, key: &str, value: &Value, theme: &Theme) {
    match value {
        Value::Null => {}
        Value::Array(items) => {
            lines.push(work_kv_line(
                key,
                &format!(
                    "{} item{}",
                    items.len(),
                    if items.len() == 1 { "" } else { "s" }
                ),
                theme,
            ));
            for item in items {
                lines.push(Line::from(vec![
                    Span::styled("    • ", theme.muted_style()),
                    Span::styled(format_work_value(item), Style::default().fg(theme.fg)),
                ]));
            }
        }
        Value::Object(map) => {
            lines.push(work_kv_line(
                key,
                &format!(
                    "{} field{}",
                    map.len(),
                    if map.len() == 1 { "" } else { "s" }
                ),
                theme,
            ));
            let mut fields = map.iter().collect::<Vec<_>>();
            fields.sort_by(|left, right| left.0.cmp(right.0));
            for (field, field_value) in fields {
                lines.push(Line::from(vec![
                    Span::styled(format!("    {field}: "), theme.muted_style()),
                    Span::styled(
                        format_work_value(field_value),
                        Style::default().fg(theme.fg),
                    ),
                ]));
            }
        }
        _ => lines.push(work_kv_line(key, &format_work_value(value), theme)),
    }
}

fn work_kv_line(key: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key}: "), theme.muted_style()),
        Span::styled(value.to_string(), Style::default().fg(theme.fg)),
    ])
}

fn push_blank_line(lines: &mut Vec<Line<'static>>) {
    if lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::raw(""));
    }
}

fn style_for_work_status(status: Option<&str>, theme: &Theme) -> Style {
    match status.unwrap_or_default() {
        "done" | "closed" | "resolved" => theme.success_style(),
        "active" | "ready" | "review" => theme.accent_style(),
        "blocked" | "failed" | "needs_context" => theme.error_style(),
        "todo" | "open" => theme.warning_style(),
        _ => theme.muted_style(),
    }
}

fn format_work_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(items) => items
            .iter()
            .map(format_work_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => {
            if let (Some(id), Some(title)) = (
                map.get("id").and_then(Value::as_str),
                map.get("title").and_then(Value::as_str),
            ) {
                return format!("{id} · {title}");
            }
            let mut fields = map
                .iter()
                .filter_map(|(key, value)| {
                    let formatted = format_work_value(value);
                    (!formatted.is_empty()).then(|| format!("{key}: {formatted}"))
                })
                .collect::<Vec<_>>();
            fields.sort();
            fields.join(" · ")
        }
    }
}
