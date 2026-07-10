use imp_core::config::AnimationLevel;
use imp_llm::truncate_chars_with_suffix;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use serde_json::Value;

use crate::animation::{static_working_glyph, tool_frame};
use crate::theme::Theme;

fn abbreviate_home_path(path: &str) -> String {
    for prefix in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if let Some((_, suffix)) = rest.split_once('/') {
                return format!("~/{suffix}");
            }
            return "~".to_string();
        }
    }
    path.to_string()
}

fn abbreviate_path(path: &str, max: usize) -> String {
    let abbreviated = abbreviate_home_path(path);
    if abbreviated.chars().count() <= max {
        return abbreviated;
    }

    let file_name = abbreviated.rsplit('/').next().unwrap_or(&abbreviated);
    let fallback = format!("…/{file_name}");
    if fallback.chars().count() <= max {
        fallback
    } else {
        truncate_chars_with_suffix(&fallback, max, "…")
    }
}

fn abbreviate_path_list(items: &[Value]) -> String {
    items
        .iter()
        .filter_map(|v| v.as_str())
        .map(abbreviate_home_path)
        .collect::<Vec<_>>()
        .join(", ")
}

fn shell_summary(args: &Value) -> String {
    args.get("command")
        .and_then(|v| v.as_str())
        .map(|command| command.trim().to_string())
        .unwrap_or_default()
}

/// A tool call ready for display.
#[derive(Debug, Clone)]
pub struct DisplayToolCall {
    pub id: String,
    pub name: String,
    pub args_summary: String,
    pub output: Option<String>,
    pub details: serde_json::Value,
    pub is_error: bool,
    pub expanded: bool,
    /// Deduplicated policy, trust, and provenance notices scoped to this tool.
    pub notices: Vec<String>,
    /// Rolling buffer of recent streaming output lines for inline chat display.
    pub streaming_lines: Vec<String>,
    /// Full streaming output collected while the tool is still running.
    pub streaming_output: String,
}

impl DisplayToolCall {
    /// Build a compact one-line summary for the tool call header.
    pub fn header_line(&self, theme: &Theme) -> Line<'static> {
        self.header_line_animated(theme, 0, AnimationLevel::Minimal)
    }

    /// Header with animated spinner for running tools.
    pub fn header_line_animated(
        &self,
        theme: &Theme,
        tick: u64,
        animation_level: AnimationLevel,
    ) -> Line<'static> {
        self.header_line_animated_focused(theme, tick, false, animation_level)
    }

    /// Header with animated spinner and optional focus indicator.
    pub fn header_line_animated_focused(
        &self,
        theme: &Theme,
        tick: u64,
        focused: bool,
        animation_level: AnimationLevel,
    ) -> Line<'static> {
        let is_running = self.output.is_none() && !self.is_error;
        let status_icon = if self.is_error {
            "✗"
        } else if is_running && animation_level == AnimationLevel::None {
            static_working_glyph()
        } else if is_running {
            tool_frame(tick)
        } else {
            "✓"
        };
        let icon_style = if self.is_error {
            theme.error_style()
        } else if is_running {
            Style::default().fg(theme.accent)
        } else {
            theme.success_style()
        };

        // Focus indicator prepended before the status icon
        let focus_span = if focused {
            Span::styled(
                "▸",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(" ")
        };

        let mut spans = vec![focus_span];
        spans.push(Span::styled(format!(" {status_icon} "), icon_style));
        let is_terminal = matches!(self.name.as_str(), "bash" | "shell");
        let tool_icon = tool_display_icon(&self.name);
        if is_terminal {
            spans.push(Span::styled(
                format_tool_icon_for_label(tool_icon, &self.name),
                Style::default().fg(theme.accent),
            ));
            if !self.args_summary.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(self.args_summary.clone(), theme.muted_style()));
            }
        } else {
            spans.push(Span::styled(
                format_tool_icon_for_label(tool_icon, &self.name),
                Style::default().fg(theme.accent),
            ));
            spans.push(Span::styled(
                tool_display_name(&self.name),
                Style::default()
                    .fg(theme.tool_name)
                    .add_modifier(Modifier::BOLD),
            ));

            if !self.args_summary.is_empty() {
                if let Some((primary, secondary)) = split_args_summary(&self.args_summary) {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(primary, theme.muted_style()));
                    if let Some(secondary) = secondary {
                        spans.push(Span::styled(format!(" · {secondary}"), theme.muted_style()));
                    }
                }
            }
        }

        if !self.notices.is_empty() {
            let count = self.notices.len();
            let label = if count == 1 {
                "  ⚠".to_string()
            } else {
                format!("  ⚠ {count}")
            };
            spans.push(Span::styled(label, theme.warning_style()));
        }

        // Result summary when collapsed — keep it compact but more useful than a raw line count.
        if !self.expanded {
            if let Some(ref output) = self.output {
                if self.is_error {
                    spans.push(Span::styled(" failed", theme.error_style()));
                } else {
                    let line_count = output.lines().count();
                    let suffix = if line_count == 1 { "line" } else { "lines" };
                    spans.push(Span::styled(
                        format!("  · {line_count} {suffix}"),
                        theme.muted_style(),
                    ));
                }
            }
        }

        Line::from(spans)
    }

    pub fn summary_detail_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let Some((_primary, Some(secondary))) = split_args_summary(&self.args_summary) else {
            return Vec::new();
        };
        vec![Line::from(vec![
            Span::styled("  └ ", theme.muted_style()),
            Span::styled(secondary, theme.muted_style()),
        ])]
    }

    pub fn add_notice(&mut self, notice: &str) {
        if !self.notices.iter().any(|existing| existing == notice) {
            self.notices.push(notice.to_string());
        }
    }

    pub fn notice_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        self.notices
            .iter()
            .map(|notice| {
                Line::from(vec![
                    Span::styled("⚠ ", theme.warning_style()),
                    Span::styled(notice.clone(), theme.muted_style()),
                ])
            })
            .collect()
    }

    /// Build compact inline spans for multi-tool-per-line rendering: "✓ name args"
    pub fn compact_spans(&self, theme: &Theme) -> Vec<Span<'static>> {
        let icon_style = theme.success_style();
        let args_short = short_args(&self.args_summary);
        let tool_icon = tool_display_icon(&self.name);
        let mut spans = vec![
            Span::styled("✓ ", icon_style),
            Span::styled(
                format_tool_icon_for_label(tool_icon, &self.name),
                Style::default().fg(theme.accent),
            ),
            Span::styled(
                tool_display_name(&self.name),
                Style::default()
                    .fg(theme.tool_name)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if !args_short.is_empty() {
            spans.push(Span::styled(format!(" {args_short}"), theme.muted_style()));
        }
        if !self.notices.is_empty() {
            let label = if self.notices.len() == 1 {
                "  ⚠".to_string()
            } else {
                format!("  ⚠ {}", self.notices.len())
            };
            spans.push(Span::styled(label, theme.warning_style()));
        }
        spans
    }

    /// Build a compact args summary from tool name and arguments.
    pub fn make_args_summary(name: &str, args: &serde_json::Value) -> String {
        match name {
            "read" => args
                .get("path")
                .and_then(|v| v.as_str())
                .map(abbreviate_home_path)
                .unwrap_or_default(),
            "bash" | "shell" => shell_summary(args),
            "edit" | "write" | "multi_edit" => format_edit_args(args),
            "scan" => format_scan_args(args),
            "workflow" => format_workflow_args(args),
            "work" => format_work_args(args),
            "prototype" => format_prototype_args(args),
            "git" => format_git_args(args),
            "web" => format_web_args(args),
            "browser" => format_browser_args(args),
            _ => summarize_json_object(args),
        }
    }
}

fn split_args_summary(summary: &str) -> Option<(String, Option<String>)> {
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    summary
        .split_once("\n  ")
        .map(|(primary, secondary)| (primary.to_string(), Some(secondary.trim().to_string())))
        .or_else(|| Some((summary.to_string(), None)))
}

fn push_named_field(fields: &mut Vec<String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        let value = truncate_chars_with_suffix(&value, 36, "…");
        if !value.is_empty() {
            fields.push(format!("{key} {value}"));
        }
    }
}

fn action_with_fields(action: &str, fields: &[String]) -> String {
    if fields.is_empty() {
        action.to_string()
    } else {
        format!("{action}  {}", fields.join("  "))
    }
}

fn format_scan_args(args: &Value) -> String {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("scan");
    match action {
        "search" => args
            .get("query")
            .and_then(value_to_short_string)
            .map(|query| format!("search {query}"))
            .unwrap_or_else(|| "search".to_string()),
        "extract" => args
            .get("targets")
            .or_else(|| args.get("files"))
            .and_then(Value::as_array)
            .map(|items| abbreviate_path_list(items))
            .unwrap_or_else(|| "extract".to_string()),
        "directory" | "scan" => args
            .get("directory")
            .and_then(Value::as_str)
            .map(abbreviate_home_path)
            .unwrap_or_default(),
        _ => {
            if action == "scan" {
                String::new()
            } else {
                action.to_string()
            }
        }
    }
}

fn format_workflow_args(args: &Value) -> String {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("workflow");
    let id = args.get("id").and_then(Value::as_str);
    let mode = args.get("mode").and_then(Value::as_str);

    match action {
        "list" => mode
            .map(|mode| format!("list · {mode}"))
            .unwrap_or_else(|| "list".to_string()),
        "show" | "validate" | "run" => {
            let mut summary = id
                .map(|id| format!("{action} {id}"))
                .unwrap_or_else(|| action.to_string());
            if let Some(mode) = mode.filter(|mode| !mode.is_empty()) {
                summary.push_str(&format!(" · {mode}"));
            }
            summary
        }
        "update" => {
            let mut summary = id
                .map(|id| format!("update {id}"))
                .unwrap_or_else(|| "update".to_string());
            if let Some(path) = args.get("path").and_then(Value::as_str) {
                summary.push_str(&format!(" · {path}"));
                if let Some(value) = args.get("value").and_then(value_to_short_string) {
                    summary.push_str(&format!(" → {value}"));
                }
            }
            if let Some(reason) = args
                .get("reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                summary.push_str("\n  ");
                summary.push_str(&truncate_chars_with_suffix(reason, 72, "…"));
            }
            summary
        }
        _ => {
            let mut fields = Vec::new();
            push_named_field(&mut fields, "id", id.map(str::to_string));
            push_named_field(&mut fields, "mode", mode.map(str::to_string));
            action_with_fields(action, &fields)
        }
    }
}

fn format_work_args(args: &Value) -> String {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("work");
    let title = args
        .get("title")
        .or_else(|| args.get("text"))
        .and_then(value_to_short_string)
        .map(|title| truncate_chars_with_suffix(&title, 44, "…"));
    let id = args.get("id").and_then(value_to_short_string);
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("item");
    let status = args.get("status").and_then(Value::as_str);
    let outcome = args.get("outcome").and_then(Value::as_str);

    match action {
        "create" => match title {
            Some(title) => format!(
                "create {kind} · {status}\n  {title}",
                status = status.unwrap_or("todo")
            ),
            None => format!("create {kind}{}", status_suffix(status)),
        },
        "close" => id
            .map(|id| format!("close {id}{}", outcome_suffix(outcome)))
            .unwrap_or_else(|| format!("close{}", outcome_suffix(outcome))),
        "update" => id
            .map(|id| format!("update {id}{}", status_suffix(status)))
            .unwrap_or_else(|| format!("update {kind}{}", status_suffix(status))),
        "show" => id
            .map(|id| format!("show {id}"))
            .unwrap_or_else(|| format!("show {kind}")),
        "list" => status
            .map(|status| format!("list {kind}s · {status}"))
            .unwrap_or_else(|| format!("list {kind}s")),
        _ => {
            let mut parts = vec![action.to_string()];
            if let Some(title) = title {
                parts.push(title);
            } else if let Some(id) = id {
                parts.push(id);
            } else if kind != "item" {
                parts.push(kind.to_string());
            }
            let suffix = status
                .or(outcome)
                .map(|value| format!(" · {value}"))
                .unwrap_or_default();
            format!("{}{}", parts.join(" "), suffix)
        }
    }
}

fn status_suffix(status: Option<&str>) -> String {
    status
        .map(|status| format!(" · {status}"))
        .unwrap_or_default()
}

fn outcome_suffix(outcome: Option<&str>) -> String {
    outcome
        .filter(|outcome| !outcome.is_empty())
        .map(|outcome| format!(" · {outcome}"))
        .unwrap_or_default()
}

fn format_prototype_args(args: &Value) -> String {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("prototype");
    let mut fields = Vec::new();
    for key in [
        "question",
        "language",
        "hypothesis_result",
        "recommended_action",
    ] {
        push_named_field(
            &mut fields,
            key,
            args.get(key).and_then(value_to_short_string),
        );
    }
    action_with_fields(action, &fields)
}

fn format_git_args(args: &Value) -> String {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("git");
    let mut fields = Vec::new();
    for key in ["base", "head", "message"] {
        push_named_field(
            &mut fields,
            key,
            args.get(key).and_then(value_to_short_string),
        );
    }
    if let Some(files) = args.get("files").and_then(Value::as_array) {
        push_named_field(&mut fields, "files", Some(abbreviate_path_list(files)));
    }
    action_with_fields(action, &fields)
}

fn format_web_args(args: &Value) -> String {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("web");
    let mut fields = Vec::new();
    for key in ["query", "url"] {
        push_named_field(
            &mut fields,
            key,
            args.get(key).and_then(value_to_short_string),
        );
    }
    action_with_fields(action, &fields)
}

fn format_browser_args(args: &Value) -> String {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("browser");
    let mut fields = Vec::new();
    for key in ["url", "selector", "session_id"] {
        push_named_field(
            &mut fields,
            key,
            args.get(key).and_then(value_to_short_string),
        );
    }
    if args.get("value").is_some() {
        fields.push("value [redacted]".into());
    }
    action_with_fields(action, &fields)
}

fn format_edit_args(args: &Value) -> String {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| first_edit_path(args))
        .map(|path| abbreviate_path(path, 32));
    let edit_count = args
        .get("edits")
        .and_then(Value::as_array)
        .map(|edits| edits.len())
        .or_else(|| args.get("old_text").map(|_| 1));

    match (path, edit_count) {
        (Some(path), Some(count)) => format!(
            "{path} · {count} change{}",
            if count == 1 { "" } else { "s" }
        ),
        (Some(path), None) => path,
        (None, Some(count)) => {
            format!("edit · {count} change{}", if count == 1 { "" } else { "s" })
        }
        (None, None) => "edit".to_string(),
    }
}

fn first_edit_path(args: &Value) -> Option<&str> {
    args.get("edits")
        .and_then(Value::as_array)
        .and_then(|edits| edits.iter().find_map(|edit| edit.get("path")?.as_str()))
}

pub fn tool_display_icon(name: &str) -> &'static str {
    match name {
        "prototype" => "⚗",
        "ask_user" => "?",
        "work" => "▣",
        "bash" | "shell" => "$",
        "read" => "◧",
        "write" => "✎",
        "edit" | "multi_edit" => "◇",
        "git" => "◆",
        "scan" => "⌕",
        "web" => "◎",
        "browser" => "◉",
        "workflow" => "⚑",
        _ => "•",
    }
}

fn format_tool_icon_for_label(icon: &str, tool_name: &str) -> String {
    if matches!(tool_name, "bash" | "shell") {
        icon.to_string()
    } else {
        format!("{icon} ")
    }
}

pub fn tool_display_name(name: &str) -> String {
    match name {
        "workflow" => "Workflow".to_string(),
        "ask_user" => "Ask".to_string(),
        "prototype" => "Prototype".to_string(),
        "bash" | "shell" => "Terminal".to_string(),
        "multi_edit" => "Edit".to_string(),
        "browser" => "Browser".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

fn summarize_json_object(args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        let json = serde_json::to_string(args).unwrap_or_default();
        return truncate_chars_with_suffix(&json, 80, "…");
    };

    let mut fields = Vec::new();
    for (key, value) in obj {
        if let Some(short) = value_to_short_string(value) {
            fields.push(format!("{key} {short}"));
        }
    }

    if fields.is_empty() {
        "{}".to_string()
    } else {
        fields.join("  ")
    }
}

fn value_to_short_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(truncate_chars_with_suffix(
            &abbreviate_home_path(s),
            32,
            "…",
        )),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(value_to_short_string)
                .collect::<Vec<_>>()
                .join(",");
            if joined.is_empty() {
                None
            } else {
                Some(truncate_chars_with_suffix(&joined, 32, "…"))
            }
        }
        Value::Object(_) => Some("{…}".to_string()),
    }
}

/// Renders a single tool call (header + optionally expanded output).
pub struct ToolCallView<'a> {
    tool_call: &'a DisplayToolCall,
    theme: &'a Theme,
}

impl<'a> ToolCallView<'a> {
    pub fn new(tool_call: &'a DisplayToolCall, theme: &'a Theme) -> Self {
        Self { tool_call, theme }
    }
}

impl Widget for ToolCallView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        // Render header line
        let header = self.tool_call.header_line(self.theme);
        buf.set_line(area.x, area.y, &header, area.width);

        // Render expanded output
        if self.tool_call.expanded {
            if let Some(ref output) = self.tool_call.output {
                let output_style = if self.tool_call.is_error {
                    self.theme.error_style()
                } else {
                    self.theme.muted_style()
                };

                for (i, line_str) in output.lines().enumerate() {
                    let y = area.y + 1 + i as u16;
                    if y >= area.y + area.height {
                        break;
                    }
                    let line = Line::from(Span::styled(format!("    {line_str}"), output_style));
                    buf.set_line(area.x, y, &line, area.width);
                }
            }
        }
    }
}

/// Calculate the rendered height of a tool call.
pub fn tool_call_height(tc: &DisplayToolCall) -> u16 {
    let mut h: u16 = 1; // header
    if tc.expanded {
        if let Some(ref output) = tc.output {
            h += output.lines().count().min(50) as u16; // cap at 50 lines
        }
    }
    h
}

/// Check whether a tool call can be rendered in compact (inline) mode.
/// Compactable = completed successfully, not expanded, not an error.
pub fn is_compactable(tc: &DisplayToolCall) -> bool {
    tc.output.is_some() && !tc.is_error && !tc.expanded
}

/// Calculate the rendered height of a slice of tool calls using compact grouping.
/// Consecutive compactable calls share lines; others get their own full-height row.
pub fn tool_calls_compact_height(tcs: &[DisplayToolCall], width: u16) -> u16 {
    let mut h: u16 = 0;
    let mut i = 0;
    while i < tcs.len() {
        let tc = &tcs[i];
        if is_compactable(tc) {
            let group_start = i;
            while i < tcs.len() && is_compactable(&tcs[i]) {
                i += 1;
            }
            h += compact_group_line_count(&tcs[group_start..i], width);
        } else {
            h += tool_call_height(tc);
            i += 1;
        }
    }
    h
}

/// Calculate how many lines a group of compact tool calls takes.
/// Each call renders as "✓ name args" and we pack as many as fit per line.
fn compact_group_line_count(tcs: &[DisplayToolCall], width: u16) -> u16 {
    if tcs.is_empty() {
        return 0;
    }
    let usable = (width as usize).saturating_sub(4); // rail = 4 chars
    if usable == 0 {
        return tcs.len() as u16;
    }
    let mut lines: u16 = 1;
    let mut col: usize = 0;
    for tc in tcs {
        let span_len = compact_span_width(tc);
        if col > 0 && col + 2 + span_len > usable {
            lines += 1;
            col = span_len;
        } else if col > 0 {
            col += 2 + span_len; // 2 for "  " separator
        } else {
            col = span_len;
        }
    }
    lines
}

/// Width of a compact tool call span: "✓ name args" character count.
fn compact_span_width(tc: &DisplayToolCall) -> usize {
    let args_short = short_args(&tc.args_summary);
    let w = 2 + tc.name.len(); // "✓ name"
    if args_short.is_empty() {
        w
    } else {
        w + 1 + args_short.len()
    }
}

/// Shorten args_summary for compact display (just the filename or first word).
fn short_args(args: &str) -> String {
    if args.is_empty() {
        return String::new();
    }
    // For paths, show just the filename
    if args.contains('/') {
        if let Some(name) = args.rsplit('/').next() {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    // For "$ command" bash summaries, take first 20 chars
    if let Some(cmd) = args.strip_prefix("$ ") {
        let short = if cmd.len() > 20 {
            format!("$ {}", truncate_chars_with_suffix(cmd, 17, "…"))
        } else {
            format!("$ {cmd}")
        };
        return short;
    }
    // For quoted grep patterns, keep as-is if short
    if args.len() <= 24 {
        return args.to_string();
    }
    truncate_chars_with_suffix(args, 21, "…")
}

#[cfg(test)]
#[path = "tools/tests.rs"]
mod tests;
