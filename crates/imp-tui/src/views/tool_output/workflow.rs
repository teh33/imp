use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::theme::Theme;
use crate::views::tools::DisplayToolCall;

use super::{
    append_card_meta, card_meta_line, plain_line_style, styled_plain_output_with, tool_card_output,
};

pub(super) fn styled_workflow_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let body = workflow_output_body(tc, theme);
    tool_card_output(
        "Workflow",
        tc.details.get("action").and_then(Value::as_str),
        "⚑",
        body,
        theme,
    )
}

fn workflow_output_body(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    if tc.details.get("action").and_then(Value::as_str) == Some("run") {
        return workflow_run_body(tc, theme);
    }

    let mut body = Vec::new();

    append_card_meta(
        &mut body,
        "workflow",
        tc.details.get("id").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "path",
        tc.details.get("path").and_then(Value::as_str),
        theme,
    );
    if let Some(value) = tc.details.get("value").and_then(workflow_value_string) {
        body.push(card_meta_line("value", &value, theme));
    }
    append_card_meta(
        &mut body,
        "reason",
        tc.details.get("reason").and_then(Value::as_str),
        theme,
    );

    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, workflow_line_style));
    body
}

fn workflow_run_body(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    let result = tc.details.get("result");
    let next_action = result.and_then(|result| result.get("next_action"));

    append_card_meta(
        &mut body,
        "workflow",
        result
            .and_then(|result| result.get("id"))
            .and_then(Value::as_str)
            .or_else(|| tc.details.get("id").and_then(Value::as_str)),
        theme,
    );
    append_card_meta(
        &mut body,
        "status",
        result
            .and_then(|result| result.get("status"))
            .and_then(Value::as_str),
        theme,
    );

    match next_action
        .and_then(|action| action.get("kind"))
        .and_then(Value::as_str)
    {
        Some("orchestrated_command_checks") => {
            let steps = next_action
                .and_then(|action| action.get("steps"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let check_count = steps
                .iter()
                .filter_map(|step| step.get("checks").and_then(Value::as_array))
                .map(Vec::len)
                .sum::<usize>();
            body.push(Line::from(vec![
                Span::styled("  ran: ", theme.muted_style()),
                Span::styled(
                    format!("{} step(s), {} check(s)", steps.len(), check_count),
                    theme.success_style(),
                ),
            ]));
            for step in steps {
                let step_id = step
                    .get("step")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let step_status = step
                    .get("step_status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                body.push(Line::from(vec![
                    Span::styled("  ✓ ", theme.success_style()),
                    Span::styled(step_id.to_string(), Style::default().fg(theme.fg)),
                    Span::styled(format!(" — {step_status}"), theme.muted_style()),
                ]));
            }
        }
        Some("agent_action") => {
            let contract = next_action.and_then(|action| action.get("contract"));
            append_card_meta(
                &mut body,
                "step",
                next_action
                    .and_then(|action| action.get("step"))
                    .and_then(Value::as_str),
                theme,
            );
            append_card_meta(
                &mut body,
                "role",
                contract
                    .and_then(|contract| contract.get("role"))
                    .and_then(Value::as_str),
                theme,
            );
            append_card_meta(
                &mut body,
                "objective",
                contract
                    .and_then(|contract| contract.get("objective"))
                    .and_then(Value::as_str),
                theme,
            );
            workflow_array_section(
                &mut body,
                "instructions",
                contract.and_then(|contract| contract.get("instructions")),
                theme,
            );
            workflow_array_section(
                &mut body,
                "allowed writes",
                contract.and_then(|contract| contract.get("write_scope")),
                theme,
            );
            workflow_array_section(
                &mut body,
                "completion checks",
                contract.and_then(|contract| contract.get("completion_checks")),
                theme,
            );
            workflow_array_section(
                &mut body,
                "completion artifacts",
                contract.and_then(|contract| contract.get("completion_artifacts")),
                theme,
            );
        }
        Some("missing_action_contract") => {
            append_card_meta(
                &mut body,
                "step",
                next_action
                    .and_then(|action| action.get("step"))
                    .and_then(Value::as_str),
                theme,
            );
            append_card_meta(
                &mut body,
                "blocked",
                next_action
                    .and_then(|action| action.get("reason"))
                    .and_then(Value::as_str),
                theme,
            );
        }
        Some("run_step") => {
            append_card_meta(
                &mut body,
                "next step",
                next_action
                    .and_then(|action| action.get("step"))
                    .and_then(Value::as_str),
                theme,
            );
            append_card_meta(
                &mut body,
                "worker",
                next_action
                    .and_then(|action| action.get("worker"))
                    .and_then(Value::as_str),
                theme,
            );
        }
        Some("validation_blocked") => {
            body.push(Line::from(vec![Span::styled(
                "  blocked by validation diagnostics",
                theme.error_style(),
            )]));
        }
        Some("no_runnable_steps") => {
            body.push(Line::from(vec![Span::styled(
                "  no runnable workflow steps",
                theme.muted_style(),
            )]));
        }
        _ => {}
    }

    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, workflow_line_style));
    body
}

fn workflow_array_section(
    body: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<&Value>,
    theme: &Theme,
) {
    let Some(items) = value
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    else {
        return;
    };
    body.push(card_meta_line(label, "", theme));
    for item in items {
        if let Some(text) = item.as_str().filter(|text| !text.is_empty()) {
            body.push(Line::from(vec![
                Span::styled("    - ", theme.muted_style()),
                Span::styled(text.to_string(), Style::default().fg(theme.fg)),
            ]));
        }
    }
}

fn workflow_value_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

fn workflow_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error || line.contains("diagnostics") && !line.contains("0 with diagnostics") {
        theme.error_style()
    } else if line.contains(": ok")
        || line.starts_with("Updated workflow")
        || line.starts_with("Validated") && line.contains("0 with diagnostics")
    {
        theme.success_style()
    } else if line.starts_with("Next workflow action") || line.starts_with("Checks:") {
        theme.accent_style()
    } else {
        plain_line_style(line, theme, is_error)
    }
}
