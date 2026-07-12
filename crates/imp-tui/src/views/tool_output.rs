use std::path::Path;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::highlight::Highlighter;
use crate::theme::Theme;
use crate::views::tools::DisplayToolCall;

mod diff;
mod work;
mod workflow;
mod wrap;
use diff::git_diff_lines_with_line_numbers;
use work::styled_work_output;
use workflow::styled_workflow_output;
pub use wrap::wrap_styled_lines;

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
        "browser" => styled_browser_output(tc, theme),
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
        "browser" => styled_browser_sidebar_output(tc, theme),
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

pub(super) fn tool_card_output(
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

pub(super) fn card_meta_line(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}: "), theme.muted_style()),
        Span::styled(value.to_string(), Style::default().fg(theme.fg)),
    ])
}

pub(super) fn append_card_meta(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: Option<&str>,
    theme: &Theme,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(card_meta_line(label, value, theme));
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

fn styled_browser_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc.details.get("action").and_then(Value::as_str);
    let location = tc
        .details
        .get("domain")
        .and_then(Value::as_str)
        .or_else(|| tc.details.get("url").and_then(Value::as_str));
    let mut spans = vec![Span::styled(
        action.unwrap_or("browser").to_string(),
        theme.accent_style(),
    )];
    if let Some(location) = location {
        spans.push(Span::styled(format!(" · {location}"), theme.muted_style()));
    }
    if let Some(sequence) = tc.details.get("sequence").and_then(Value::as_u64) {
        spans.push(Span::styled(format!(" · #{sequence}"), theme.muted_style()));
    }
    vec![Line::from(spans)]
}

fn styled_browser_sidebar_output(tc: &DisplayToolCall, theme: &Theme) -> Vec<Line<'static>> {
    let action = tc.details.get("action").and_then(Value::as_str);
    let mut body = Vec::new();
    append_card_meta(
        &mut body,
        "session",
        tc.details.get("session_id").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "domain",
        tc.details.get("domain").and_then(Value::as_str),
        theme,
    );
    append_card_meta(
        &mut body,
        "url",
        tc.details.get("url").and_then(Value::as_str),
        theme,
    );
    if let Some(sequence) = tc.details.get("sequence").and_then(Value::as_u64) {
        body.push(card_meta_line("sequence", &sequence.to_string(), theme));
    }
    if let Some(elements) = tc
        .details
        .get("interactive_elements")
        .and_then(Value::as_u64)
    {
        body.push(card_meta_line("elements", &elements.to_string(), theme));
    }
    if !body.is_empty() {
        body.push(Line::raw(""));
    }
    body.extend(styled_plain_output_with(tc, theme, web_line_style));
    tool_card_output("Browser · Lightpanda", action, "◉", body, theme)
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

pub(super) fn styled_plain_output_with(
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

pub(super) fn tool_output_text(tc: &DisplayToolCall) -> Option<&str> {
    tc.output.as_deref().or({
        if tc.streaming_output.is_empty() {
            None
        } else {
            Some(tc.streaming_output.as_str())
        }
    })
}

pub(super) fn plain_line_style(_line: &str, theme: &Theme, is_error: bool) -> Style {
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

pub(super) fn git_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
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

#[cfg(test)]
#[path = "tool_output/tests.rs"]
mod tests;
