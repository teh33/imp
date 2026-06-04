use std::path::Path;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::highlight::Highlighter;
use crate::theme::Theme;
use crate::views::tools::DisplayToolCall;

pub fn styled_tool_output_lines(
    tc: &DisplayToolCall,
    highlighter: &Highlighter,
    theme: &Theme,
    with_line_numbers: bool,
) -> Vec<Line<'static>> {
    match tc.name.as_str() {
        "read" => styled_read_output(tc, highlighter, theme, with_line_numbers),
        "write" => styled_write_output(tc, highlighter, theme),
        "edit" | "multi_edit" => styled_diff_output(tc, theme),
        "bash" | "shell" => styled_terminal_output(tc, theme),
        "git" => styled_git_output(tc, theme),
        "scan" => styled_scan_output(tc, theme),
        "workflow" => styled_workflow_output(tc, theme),
        "work" => styled_work_output(tc, theme),
        "web" => styled_web_output(tc, theme),
        "ask_user" | "extend" | "audit_scan" | "openrouter_secret_run" => {
            styled_status_output(tc, theme)
        }
        "color_palette" => styled_palette_output(tc, theme),
        _ => styled_plain_output(tc, theme),
    }
}

pub fn styled_sidebar_tool_output_lines(
    tc: &DisplayToolCall,
    _highlighter: &Highlighter,
    theme: &Theme,
    with_line_numbers: bool,
) -> Vec<Line<'static>> {
    if tc.name == "git" && tc.details.get("action").and_then(|v| v.as_str()) == Some("diff") {
        return styled_git_diff_output_with_line_numbers(tc, theme);
    }

    match tc.name.as_str() {
        "bash" | "shell" => styled_shell_sidebar_output(tc, theme),
        "git" => styled_git_sidebar_output(tc, theme),
        "scan" => styled_scan_sidebar_output(tc, theme),
        "workflow" => styled_workflow_output(tc, theme),
        "web" => styled_web_sidebar_output(tc, theme),
        "prototype" => styled_prototype_sidebar_output(tc, theme),
        "work" => styled_work_output(tc, theme),
        "read" => styled_read_sidebar_output(tc, _highlighter, theme, with_line_numbers),
        "write" => styled_write_sidebar_output(tc, _highlighter, theme),
        "edit" | "multi_edit" => styled_edit_sidebar_output(tc, theme),
        "ask_user" | "extend" | "audit_scan" | "openrouter_secret_run" => tool_card_output(
            &tc.name,
            None,
            "•",
            styled_plain_output_with(tc, theme, status_line_style),
            theme,
        ),
        _ => tool_card_output(
            &tc.name,
            None,
            "•",
            styled_plain_output_with(tc, theme, plain_line_style),
            theme,
        ),
    }
}

pub fn wrap_styled_lines(lines: &[Line<'static>], width: usize) -> Vec<Line<'static>> {
    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap_line(line, width));
    }
    wrapped
}

fn styled_read_output(
    tc: &DisplayToolCall,
    highlighter: &Highlighter,
    theme: &Theme,
    with_line_numbers: bool,
) -> Vec<Line<'static>> {
    let Some(output) = tool_output_text(tc) else {
        return vec![Line::from(Span::styled("Running…", theme.muted_style()))];
    };

    let total_code_lines = tc
        .details
        .get("lines")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or_else(|| output.lines().count());

    let all_lines: Vec<&str> = output.lines().collect();
    let code_lines = all_lines
        .iter()
        .take(total_code_lines)
        .copied()
        .collect::<Vec<_>>();
    let extra_lines = all_lines
        .iter()
        .skip(total_code_lines)
        .copied()
        .collect::<Vec<_>>();

    let code = code_lines.join("\n");
    let path = tc
        .details
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(&tc.args_summary);
    let language = language_token_from_path(path);

    let mut rendered =
        highlight_code_lines(highlighter, &code, &language, with_line_numbers, theme);
    for line in extra_lines {
        rendered.push(Line::from(Span::styled(
            line.to_string(),
            theme.muted_style(),
        )));
    }

    if rendered.is_empty() {
        vec![Line::from(Span::styled(
            "(empty file)",
            theme.muted_style(),
        ))]
    } else {
        rendered
    }
}

fn styled_write_output(
    tc: &DisplayToolCall,
    highlighter: &Highlighter,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let summary = tc
        .details
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| tc.output.as_deref().and_then(|out| out.lines().next()))
        .unwrap_or("Write completed");

    let warnings = tc
        .details
        .get("warnings")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let display_content = tc
        .details
        .get("display_content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let display_note = tc
        .details
        .get("display_note")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let path = tc
        .details
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(&tc.args_summary);
    let language = language_token_from_path(path);

    let mut rendered = vec![Line::from(Span::styled(
        summary.to_string(),
        Style::default().fg(theme.fg),
    ))];

    for warning in warnings {
        rendered.push(Line::from(Span::styled(warning, theme.warning_style())));
    }

    if display_content.is_empty() {
        rendered.push(Line::from(Span::styled(
            "(empty file)",
            theme.muted_style(),
        )));
    } else {
        rendered.extend(highlight_code_lines(
            highlighter,
            display_content,
            &language,
            false,
            theme,
        ));
    }

    if !display_note.is_empty() {
        rendered.push(Line::raw(""));
        rendered.push(Line::from(Span::styled(
            display_note.to_string(),
            theme.muted_style(),
        )));
    }

    rendered
}

fn tool_card_output(
    title: &str,
    action: Option<&str>,
    icon: &str,
    body: Vec<Line<'static>>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(icon.to_string(), theme.accent_style()),
        Span::styled(title.to_string(), theme.header_style()),
        action
            .filter(|action| !action.is_empty())
            .map(|action| Span::styled(format!(" · {action}"), theme.muted_style()))
            .unwrap_or_else(|| Span::raw(String::new())),
    ])];
    if !body.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(body);
    }
    lines
}

fn card_meta_line(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}: "), theme.muted_style()),
        Span::styled(value.to_string(), Style::default().fg(theme.fg)),
    ])
}

fn append_card_meta(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<&str>,
    theme: &Theme,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(card_meta_line(label, value, theme));
    }
}

fn styled_workflow_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
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

fn styled_shell_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "command",
        tc.details.get("command").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "workdir",
        tc.details.get("workdir").and_then(Value::as_str),
        theme,
    );
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, terminal_line_style));
    tool_card_output("Terminal", None, "$", body, theme)
}

fn styled_git_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc.details.get("action").and_then(Value::as_str);
    let mut body = Vec::new();
    for key in ["base", "head"] {
        append_card_meta(
            &mut body,
            key,
            tc.details.get(key).and_then(Value::as_str),
            theme,
        );
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, git_line_style));
    tool_card_output("Git", action, "◆", body, theme)
}

fn styled_scan_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc.details.get("action").and_then(Value::as_str);
    let mut body = Vec::new();
    for key in ["query", "target", "directory"] {
        append_card_meta(
            &mut body,
            key,
            tc.details.get(key).and_then(Value::as_str),
            theme,
        );
    }
    if let Some(files) = tc.details.get("files").and_then(Value::as_array) {
        let value = files
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        append_card_meta(&mut body, "files", Some(&value), theme);
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, scan_line_style));
    tool_card_output("Scan", action, "⌕", body, theme)
}

fn styled_web_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc.details.get("action").and_then(Value::as_str);
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "query",
        tc.details.get("query").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "url",
        tc.details.get("url").and_then(Value::as_str),
        theme,
    );
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, web_line_style));
    tool_card_output("Web", action, "◎", body, theme)
}

fn styled_prototype_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "question",
        tc.details.get("question").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "language",
        tc.details.get("language").and_then(Value::as_str),
        theme,
    );
    if let Some(exit_code) = tc.details.get("exit_code").and_then(Value::as_i64) {
        body.push(card_meta_line("exit", &exit_code.to_string(), theme));
    }
    append_card_meta(
        &mut body,
        "outcome",
        tc.details.get("outcome").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "hypothesis",
        tc.details.get("hypothesis_result").and_then(Value::as_str),
        theme,
    );
    if let Some(sandbox) = tc.details.get("sandbox").and_then(Value::as_str) {
        append_card_meta(&mut body, "sandbox", Some(sandbox), theme);
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, terminal_line_style));
    tool_card_output(
        "Prototype",
        tc.details.get("action").and_then(Value::as_str),
        "⚗",
        body,
        theme,
    )
}

fn styled_read_sidebar_output(
    tc: &DisplayToolCall,
    highlighter: &Highlighter,
    theme: &Theme,
    with_line_numbers: bool,
) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "path",
        tc.details.get("path").and_then(Value::as_str),
        theme,
    );
    if let Some(lines) = tc.details.get("lines").and_then(Value::as_u64) {
        body.push(card_meta_line("lines", &lines.to_string(), theme));
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_read_output(
        tc,
        highlighter,
        theme,
        with_line_numbers,
    ));
    tool_card_output("Read", None, "◧", body, theme)
}

fn styled_write_sidebar_output(
    tc: &DisplayToolCall,
    highlighter: &Highlighter,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "path",
        tc.details.get("path").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "mode",
        tc.details.get("mode").and_then(Value::as_str),
        theme,
    );
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_write_output(tc, highlighter, theme));
    tool_card_output("Write", None, "✎", body, theme)
}

fn styled_edit_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "path",
        tc.details.get("path").and_then(Value::as_str),
        theme,
    );
    if let Some(edits) = tc.details.get("edits").and_then(Value::as_array) {
        body.push(card_meta_line(
            "edits",
            &format!(
                "{} change{}",
                edits.len(),
                if edits.len() == 1 { "" } else { "s" }
            ),
            theme,
        ));
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_diff_output(tc, theme));
    tool_card_output("Edit", None, "◇", body, theme)
}

fn styled_diff_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let Some(output) = tc.output.as_deref().or({
        if tc.streaming_output.is_empty() {
            None
        } else {
            Some(tc.streaming_output.as_str())
        }
    }) else {
        return vec![Line::from(Span::styled("Running…", theme.muted_style()))];
    };

    let mut rendered = Vec::new();
    for line in output.lines() {
        rendered.push(styled_diff_line(line, theme, tc.is_error));
    }

    if rendered.is_empty() {
        vec![Line::from(Span::styled("(no output)", theme.muted_style()))]
    } else {
        rendered
    }
}

fn styled_terminal_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, terminal_line_style)
}

fn styled_git_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, git_line_style)
}

fn styled_git_diff_output_with_line_numbers(
    tc: &DisplayToolCall,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(output) = tool_output_text(tc) else {
        return vec![Line::from(Span::styled("Running…", theme.muted_style()))];
    };

    let lines = git_diff_lines_with_line_numbers(output, theme, tc.is_error);
    if lines.is_empty() {
        vec![Line::from(Span::styled("(no output)", theme.muted_style()))]
    } else {
        lines
    }
}

fn styled_scan_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, scan_line_style)
}

fn styled_work_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
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

fn styled_web_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, web_line_style)
}

fn styled_palette_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, palette_line_style)
}

fn styled_status_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, status_line_style)
}

fn styled_plain_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    styled_plain_output_with(tc, theme, plain_line_style)
}

fn styled_plain_output_with(
    tc: &DisplayToolCall,
    theme: &Theme,
    style_for_line: fn(&str, &Theme, bool) -> Style,
) -> Vec<Line<'static>> {
    let Some(output) = tool_output_text(tc) else {
        return vec![Line::from(Span::styled("Running…", theme.muted_style()))];
    };

    let rendered: Vec<Line<'static>> = output
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                style_for_line(line, theme, tc.is_error),
            ))
        })
        .collect();

    if rendered.is_empty() {
        vec![Line::from(Span::styled("(no output)", theme.muted_style()))]
    } else {
        rendered
    }
}

fn tool_output_text(tc: &DisplayToolCall) -> Option<&str> {
    tc.output.as_deref().or({
        if tc.streaming_output.is_empty() {
            None
        } else {
            Some(tc.streaming_output.as_str())
        }
    })
}

fn git_diff_lines_with_line_numbers(
    output: &str,
    theme: &Theme,
    is_error: bool,
) -> Vec<Line<'static>> {
    let mut old_line: Option<usize> = None;
    let mut new_line: Option<usize> = None;
    let mut rendered = Vec::new();

    for raw_line in output.lines() {
        if let Some((old_start, new_start)) = parse_unified_hunk_header(raw_line) {
            old_line = Some(old_start);
            new_line = Some(new_start);
            rendered.push(Line::from(Span::styled(
                raw_line.to_string(),
                git_line_style(raw_line, theme, is_error),
            )));
            continue;
        }

        if is_diff_metadata_line(raw_line) || old_line.is_none() || new_line.is_none() {
            rendered.push(Line::from(Span::styled(
                raw_line.to_string(),
                git_line_style(raw_line, theme, is_error),
            )));
            continue;
        }

        let (old_label, new_label, advance_old, advance_new) = if raw_line.starts_with('+') {
            (String::new(), format_line_number(new_line), false, true)
        } else if raw_line.starts_with('-') {
            (format_line_number(old_line), String::new(), true, false)
        } else if raw_line.starts_with('\\') {
            (String::new(), String::new(), false, false)
        } else {
            (
                format_line_number(old_line),
                format_line_number(new_line),
                true,
                true,
            )
        };

        let content_style = git_line_style(raw_line, theme, is_error);
        rendered.push(Line::from(vec![
            Span::styled(format!("{old_label:>4}"), theme.muted_style()),
            Span::styled("│", theme.muted_style()),
            Span::styled(format!("{new_label:>4}"), theme.muted_style()),
            Span::styled("│ ", theme.muted_style()),
            Span::styled(raw_line.to_string(), content_style),
        ]));

        if advance_old {
            old_line = old_line.map(|line| line + 1);
        }
        if advance_new {
            new_line = new_line.map(|line| line + 1);
        }
    }

    rendered
}

fn format_line_number(line: Option<usize>) -> String {
    line.map(|line| line.to_string()).unwrap_or_default()
}

fn is_diff_metadata_line(line: &str) -> bool {
    line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
}

fn parse_unified_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_range, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new_range, _) = rest.split_once(' ')?;

    Some((parse_hunk_start(old_range)?, parse_hunk_start(new_range)?))
}

fn parse_hunk_start(range: &str) -> Option<usize> {
    range
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(range)
        .parse()
        .ok()
}

fn plain_line_style(_line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        theme.error_style()
    } else {
        Style::default().fg(theme.fg)
    }
}

fn terminal_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return theme.error_style();
    }

    let trimmed = line.trim_start();
    if trimmed.starts_with("error") || trimmed.starts_with("Error") || trimmed.starts_with("FAIL") {
        theme.error_style()
    } else if trimmed.starts_with("warning")
        || trimmed.starts_with("Warning")
        || trimmed.starts_with("WARN")
    {
        theme.warning_style()
    } else if trimmed.starts_with("ok")
        || trimmed.starts_with("PASS")
        || trimmed.contains(" finished ")
        || trimmed.contains(" passed")
    {
        theme.success_style()
    } else if trimmed.starts_with('$') || trimmed.starts_with('>') {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn git_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return theme.error_style();
    }

    if line.starts_with('+') && !line.starts_with("+++") {
        theme.success_style()
    } else if line.starts_with('-') && !line.starts_with("---") {
        theme.error_style()
    } else if line.starts_with("@@")
        || line.starts_with("diff --git")
        || line.starts_with("commit ")
        || line.starts_with("On branch ")
    {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("modified:")
        || line.starts_with("new file:")
        || line.starts_with("deleted:")
        || line.contains("Changes")
    {
        theme.warning_style()
    } else {
        Style::default().fg(theme.fg)
    }
}

fn scan_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return theme.error_style();
    }

    if line.ends_with(":") || line.starts_with("Action:") || line.starts_with("Task:") {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if line.trim_start().starts_with('-')
        || line.contains("Functions")
        || line.contains("Types")
    {
        Style::default().fg(theme.tool_name)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn web_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return theme.error_style();
    }

    let trimmed = line.trim_start();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") || line.contains("://") {
        Style::default().fg(theme.accent)
    } else if line.starts_with('#') || line.ends_with(':') {
        Style::default()
            .fg(theme.tool_name)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn palette_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return theme.error_style();
    }

    if line.contains('#') || line.contains("oklch") || line.contains("rgb") {
        Style::default().fg(theme.accent)
    } else if line.ends_with(':') {
        Style::default()
            .fg(theme.tool_name)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn status_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error || line.contains("error") || line.contains("failed") {
        theme.error_style()
    } else if line.contains("success") || line.contains("completed") || line.contains("created") {
        theme.success_style()
    } else if line.contains("warning") || line.contains("skipped") {
        theme.warning_style()
    } else if line.ends_with(':') {
        Style::default()
            .fg(theme.tool_name)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    }
}

fn highlight_code_lines(
    highlighter: &Highlighter,
    code: &str,
    language: &str,
    with_line_numbers: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if code.is_empty() {
        return Vec::new();
    }

    let highlighted = highlighter.highlight_code(code, language);
    if !with_line_numbers {
        return highlighted;
    }

    highlighted
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let mut spans = vec![Span::styled(
                compact_line_number_prefix(idx + 1),
                theme.muted_style(),
            )];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

fn compact_line_number_prefix(line_number: usize) -> String {
    if line_number <= 999 {
        format!("{line_number:>3}│")
    } else {
        format!("{line_number}│")
    }
}

fn styled_diff_line(line: &str, theme: &Theme, is_error: bool) -> Line<'static> {
    let style = if line.starts_with("@@") {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with("+++") || line.starts_with("---") {
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD)
    } else if line.starts_with('+') {
        theme.success_style()
    } else if line.starts_with('-') {
        theme.error_style()
    } else if line.starts_with("Hunk ") {
        Style::default().fg(theme.accent)
    } else if line.starts_with("Warning:") {
        theme.warning_style()
    } else if is_error {
        theme.error_style()
    } else {
        Style::default().fg(theme.fg)
    };

    Line::from(Span::styled(line.to_string(), style))
}

fn language_token_from_path(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "txt".to_string())
}

fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::raw(String::new())];
    }

    let chars = flatten_line_chars(line);
    if chars.is_empty() {
        return vec![Line::raw(String::new())];
    }

    let chunks = wrap_styled_chars(&chars, width.max(1));
    chunks
        .into_iter()
        .map(|chunk| Line::from(chars_to_spans(&chunk)))
        .collect()
}

fn flatten_line_chars(line: &Line<'static>) -> Vec<(char, Style)> {
    let mut chars = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            chars.push((ch, span.style));
        }
    }
    chars
}

fn wrap_styled_chars(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let width = width.max(1);

    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= width {
            chunks.push(chars[start..].to_vec());
            break;
        }

        let end = start + width;
        let break_at = (start + 1..end)
            .rev()
            .find(|&idx| chars[idx].0.is_whitespace());

        if let Some(space_idx) = break_at {
            chunks.push(chars[start..space_idx].to_vec());
            start = space_idx + 1;
            while start < chars.len() && chars[start].0.is_whitespace() {
                start += 1;
            }
        } else {
            chunks.push(chars[start..end].to_vec());
            start = end;
        }
    }

    if chunks.is_empty() {
        chunks.push(Vec::new());
    }

    chunks
}

fn chars_to_spans(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    if chars.is_empty() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut current_style = chars[0].1;
    let mut current_text = String::new();

    for (ch, style) in chars {
        if *style == current_style {
            current_text.push(*ch);
        } else {
            spans.push(Span::styled(current_text, current_style));
            current_text = ch.to_string();
            current_style = *style;
        }
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, current_style));
    }

    spans
}

#[cfg(test)]
#[path = "tool_output/tests.rs"]
mod tests;
