use imp_core::config::{AnimationLevel, SidebarStyle, ToolOutputDisplay, UiConfig};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use serde_json::Value;

use crate::highlight::Highlighter;
use crate::selection::TextSurface;
use crate::theme::Theme;
use crate::views::tool_output::{styled_sidebar_tool_output_lines, wrap_styled_lines};
use crate::views::tools::DisplayToolCall;

#[derive(Debug, Clone)]
pub struct SidebarDetailRenderData {
    pub lines: Vec<Line<'static>>,
    pub plain_lines: Vec<String>,
}

// ── Sidebar state ───────────────────────────────────────────────

/// Sidebar state tracked in App.
#[derive(Default)]
pub struct Sidebar {
    /// Whether the sidebar pane is visible.
    pub open: bool,
    /// Scroll offset for the tool list pane (split mode, 0 = top).
    pub list_scroll: usize,
    /// Scroll offset for the detail/stream pane (0 = top).
    pub detail_scroll: usize,
    /// Whether the first tool has been seen (for auto-open logic).
    pub first_tool_seen: bool,
    /// Cached list pane height from last render (for scroll bounds).
    pub list_height: u16,
}

impl Sidebar {
    /// Reset detail scroll (call when selection changes).
    pub fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
    }

    /// Scroll the tool list up (toward earlier entries).
    pub fn scroll_list_up(&mut self, n: usize) {
        self.list_scroll = self.list_scroll.saturating_sub(n);
    }

    /// Scroll the tool list down (toward later entries).
    pub fn scroll_list_down(&mut self, n: usize) {
        self.list_scroll += n;
    }

    /// Scroll the detail/stream pane up (toward earlier content).
    pub fn scroll_detail_up(&mut self, n: usize) {
        self.detail_scroll = self.detail_scroll.saturating_sub(n);
    }

    /// Scroll the detail/stream pane down (toward later content).
    pub fn scroll_detail_down(&mut self, n: usize) {
        self.detail_scroll += n;
    }

    /// Ensure the selected tool call index is visible in the list (split mode).
    pub fn ensure_selected_visible(&mut self, selected: usize) {
        let visible = (self.list_height as usize).max(1);
        if selected < self.list_scroll {
            self.list_scroll = selected;
        } else if selected >= self.list_scroll + visible {
            self.list_scroll = selected.saturating_sub(visible.saturating_sub(1));
        }
    }
}

// ── Layout computation ──────────────────────────────────────────

/// Compute sidebar sub-areas for external hit-testing.
/// Returns `(top_hit_rect, bottom_hit_rect)` in screen coordinates.
/// In stream mode, top covers the full sidebar (bottom is zero-height).
/// In split mode, top = list area, bottom = detail area.
pub fn sidebar_sub_areas(
    sidebar_area: Rect,
    tool_count: usize,
    style: SidebarStyle,
) -> (Rect, Rect) {
    let content = Rect {
        x: sidebar_area.x + 2,
        y: sidebar_area.y,
        width: sidebar_area.width.saturating_sub(2),
        height: sidebar_area.height,
    };

    match style {
        SidebarStyle::Inspector => {
            let full = Rect {
                x: sidebar_area.x,
                width: sidebar_area.width,
                ..content
            };
            (full, full)
        }
        SidebarStyle::Stream => {
            // Stream: single scrollable pane — top covers everything
            let full = Rect {
                x: sidebar_area.x,
                width: sidebar_area.width,
                ..content
            };
            let empty = Rect {
                x: sidebar_area.x,
                width: sidebar_area.width,
                y: sidebar_area.y + sidebar_area.height,
                height: 0,
            };
            (full, empty)
        }
        SidebarStyle::Split => {
            let (list_area, _, detail_area) = compute_split(content, tool_count);
            (
                Rect {
                    x: sidebar_area.x,
                    width: sidebar_area.width,
                    y: list_area.y,
                    height: list_area.height,
                },
                Rect {
                    x: sidebar_area.x,
                    width: sidebar_area.width,
                    y: detail_area.y,
                    height: detail_area.height,
                },
            )
        }
    }
}

/// Split-mode layout: list, separator, detail areas.
fn compute_split(content: Rect, tool_count: usize) -> (Rect, Option<u16>, Rect) {
    let h = content.height as usize;
    let min_detail = 3;
    let sep = 1;
    let min_total = 2 + sep + min_detail;

    if h < min_total || tool_count == 0 {
        return (
            content,
            None,
            Rect {
                x: content.x,
                y: content.y + content.height,
                width: content.width,
                height: 0,
            },
        );
    }

    let max_list = (h * 40 / 100).max(2);
    let available_for_list = h.saturating_sub(sep + min_detail);
    let desired = tool_count.clamp(2, max_list);
    let list_h = desired.min(available_for_list).max(2);
    let detail_h = h.saturating_sub(list_h + sep);

    let list_area = Rect {
        height: list_h as u16,
        ..content
    };
    let sep_y = content.y + list_h as u16;
    let detail_area = Rect {
        y: sep_y + sep as u16,
        height: detail_h as u16,
        ..content
    };

    (list_area, Some(sep_y), detail_area)
}

// ── SidebarView widget ──────────────────────────────────────────

/// Widget that renders the sidebar in either stream or split mode.
pub struct SidebarView<'a> {
    tool_calls: Vec<&'a DisplayToolCall>,
    selected: Option<usize>,
    theme: &'a Theme,
    highlighter: &'a Highlighter,
    tick: u64,
    list_scroll: usize,
    detail_scroll: usize,
    ui_config: &'a UiConfig,
    precomputed_stream_lines: Option<&'a [Line<'static>]>,
    precomputed_detail_lines: Option<&'a [Line<'static>]>,
}

impl<'a> SidebarView<'a> {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tool_calls: Vec<&'a DisplayToolCall>,
        selected: Option<usize>,
        theme: &'a Theme,
        highlighter: &'a Highlighter,
        tick: u64,
        list_scroll: usize,
        detail_scroll: usize,
        ui_config: &'a UiConfig,
    ) -> Self {
        Self {
            tool_calls,
            selected,
            theme,
            highlighter,
            tick,
            list_scroll,
            detail_scroll,
            ui_config,
            precomputed_stream_lines: None,
            precomputed_detail_lines: None,
        }
    }

    pub fn precomputed_stream_lines(mut self, lines: &'a [Line<'static>]) -> Self {
        self.precomputed_stream_lines = Some(lines);
        self
    }

    pub fn precomputed_detail_lines(mut self, lines: &'a [Line<'static>]) -> Self {
        self.precomputed_detail_lines = Some(lines);
        self
    }
}

impl Widget for SidebarView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 2 {
            return;
        }

        // Left border separator
        let border_style = self.theme.border_style();
        for y in area.y..area.y + area.height {
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                cell.set_symbol("│");
                cell.set_style(border_style);
            }
        }

        let cx = area.x + 2;
        let cw = area.width.saturating_sub(2);
        if cw == 0 {
            return;
        }
        let content = Rect {
            x: cx,
            y: area.y,
            width: cw,
            height: area.height,
        };

        if self.tool_calls.is_empty() {
            let line = Line::from(Span::styled("No tool calls", self.theme.muted_style()));
            buf.set_line(cx, area.y, &line, cw);
            return;
        }

        match self.ui_config.sidebar_style {
            SidebarStyle::Inspector => {
                let selected_tc = self.selected.and_then(|i| self.tool_calls.get(i)).copied();
                if let Some(lines) = self.precomputed_detail_lines {
                    render_detail_from_lines(lines, self.theme, self.detail_scroll, content, buf);
                } else {
                    render_detail(
                        selected_tc,
                        self.theme,
                        self.highlighter,
                        self.detail_scroll,
                        self.ui_config,
                        content,
                        buf,
                    );
                }
            }
            SidebarStyle::Stream => {
                if let Some(lines) = self.precomputed_stream_lines {
                    render_stream_from_lines(lines, self.theme, self.detail_scroll, content, buf);
                } else {
                    render_stream(
                        &self.tool_calls,
                        self.selected,
                        self.theme,
                        self.highlighter,
                        self.tick,
                        self.detail_scroll,
                        self.ui_config,
                        content,
                        buf,
                        self.ui_config.animations,
                    );
                }
            }
            SidebarStyle::Split => {
                let (list_area, sep_y, detail_area) = compute_split(content, self.tool_calls.len());

                render_list(
                    &self.tool_calls,
                    self.selected,
                    self.theme,
                    self.tick,
                    self.list_scroll,
                    list_area,
                    buf,
                    self.ui_config.animations,
                );

                if let Some(sy) = sep_y {
                    let sep: String = "─".repeat(cw as usize);
                    buf.set_line(cx, sy, &Line::from(Span::styled(sep, border_style)), cw);
                }

                let selected_tc = self.selected.and_then(|i| self.tool_calls.get(i)).copied();
                if let Some(lines) = self.precomputed_detail_lines {
                    render_detail_from_lines(
                        lines,
                        self.theme,
                        self.detail_scroll,
                        detail_area,
                        buf,
                    );
                } else {
                    render_detail(
                        selected_tc,
                        self.theme,
                        self.highlighter,
                        self.detail_scroll,
                        self.ui_config,
                        detail_area,
                        buf,
                    );
                }
            }
        }
    }
}

// ── Stream mode rendering ───────────────────────────────────────

fn render_scrolled_lines(lines: &[Line<'_>], area: Rect, buf: &mut Buffer, scroll: usize) -> usize {
    let total = lines.len();
    let visible = area.height as usize;
    let start = scroll.min(total.saturating_sub(visible));

    for (i, line) in lines.iter().skip(start).take(visible).enumerate() {
        let row = area.y + i as u16;
        buf.set_line(area.x, row, line, area.width);
    }

    total
}

#[allow(clippy::too_many_arguments)]
pub fn build_stream_lines(
    tool_calls: &[&DisplayToolCall],
    selected: Option<usize>,
    theme: &Theme,
    highlighter: &Highlighter,
    tick: u64,
    ui_config: &UiConfig,
    animation_level: AnimationLevel,
    width: usize,
) -> Vec<Line<'static>> {
    let mut all_lines: Vec<Line<'static>> = Vec::new();

    for (idx, tc) in tool_calls.iter().enumerate() {
        let focused = selected == Some(idx);
        let header = tc.header_line_animated_focused(theme, tick, focused, animation_level);
        all_lines.push(header);
        if focused && width > 0 {
            all_lines.push(Line::from(Span::styled(
                "▸ inspector".to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let output_lines = styled_output_lines(tc, ui_config, highlighter, theme, width);
        for line in output_lines {
            all_lines.push(indent_line(line));
        }

        if idx + 1 < tool_calls.len() {
            all_lines.push(Line::raw(""));
        }
    }

    all_lines
}

fn scroll_position_indicator(total: usize, visible: usize, start: usize) -> Option<String> {
    if total <= visible || visible == 0 {
        return None;
    }

    let above = start;
    let below = total.saturating_sub(start + visible);
    let mut parts = Vec::new();
    if above > 0 {
        parts.push(format!("↑{above}"));
    }
    if below > 0 {
        parts.push(format!("↓{below}"));
    }
    (!parts.is_empty()).then(|| format!(" {} ", parts.join(" ")))
}

fn render_scroll_position_indicator(
    lines: &[Line<'_>],
    theme: &Theme,
    area: Rect,
    buf: &mut Buffer,
    scroll: usize,
) {
    let total = lines.len();
    let visible = area.height as usize;
    let start = scroll.min(total.saturating_sub(visible));
    let Some(indicator) = scroll_position_indicator(total, visible, start) else {
        return;
    };

    let iw = indicator.len() as u16;
    if area.width > iw {
        let ix = area.x + area.width - iw;
        let iy = area.y + area.height.saturating_sub(1);
        buf.set_line(
            ix,
            iy,
            &Line::from(Span::styled(indicator, theme.muted_style())),
            iw,
        );
    }
}

pub fn render_stream_from_lines(
    lines: &[Line<'_>],
    theme: &Theme,
    scroll: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    render_scrolled_lines(lines, area, buf, scroll);
    render_scroll_position_indicator(lines, theme, area, buf, scroll);
}

/// Render the sidebar as a single chronological stream of tool calls
/// with their results shown inline underneath each header.
#[allow(clippy::too_many_arguments)]
fn render_stream(
    tool_calls: &[&DisplayToolCall],
    selected: Option<usize>,
    theme: &Theme,
    highlighter: &Highlighter,
    tick: u64,
    scroll: usize,
    ui_config: &UiConfig,
    area: Rect,
    buf: &mut Buffer,
    animation_level: AnimationLevel,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let width = area.width as usize;
    let all_lines = build_stream_lines(
        tool_calls,
        selected,
        theme,
        highlighter,
        tick,
        ui_config,
        animation_level,
        width,
    );

    render_stream_from_lines(&all_lines, theme, scroll, area, buf);
}

// ── Split mode: tool list ───────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_list(
    tool_calls: &[&DisplayToolCall],
    selected: Option<usize>,
    theme: &Theme,
    tick: u64,
    scroll: usize,
    area: Rect,
    buf: &mut Buffer,
    animation_level: AnimationLevel,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let visible = area.height as usize;
    let total = tool_calls.len();
    let start = scroll.min(total.saturating_sub(visible));

    for (i, tc) in tool_calls.iter().skip(start).take(visible).enumerate() {
        let idx = start + i;
        let focused = selected == Some(idx);
        let row = area.y + i as u16;
        let header = tc.header_line_animated_focused(theme, tick, focused, animation_level);
        buf.set_line(area.x, row, &header, area.width);
        if focused && area.width > 0 {
            buf.set_string(
                area.x,
                row,
                "▸",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    if let Some(indicator) = scroll_position_indicator(total, visible, start) {
        let iw = indicator.len() as u16;
        if area.width > iw {
            let ix = area.x + area.width - iw;
            let iy = area.y + area.height.saturating_sub(1);
            buf.set_line(
                ix,
                iy,
                &Line::from(Span::styled(indicator, theme.muted_style())),
                iw,
            );
        }
    }
}

// ── Split mode: detail pane ─────────────────────────────────────

pub fn build_detail_render_data(
    tc: Option<&DisplayToolCall>,
    ui_config: &UiConfig,
    highlighter: &Highlighter,
    theme: &Theme,
    content_w: usize,
) -> SidebarDetailRenderData {
    let lines = styled_detail_lines(tc, ui_config, highlighter, theme, content_w);
    let plain_lines = lines.iter().map(line_to_plain_text).collect();
    SidebarDetailRenderData { lines, plain_lines }
}

pub fn build_detail_text_surface_from_plain_lines(
    lines: &[String],
    area: Rect,
    scroll: usize,
) -> TextSurface {
    if area.height == 0 || area.width == 0 {
        return TextSurface::new(
            crate::selection::SelectablePane::SidebarDetail,
            area,
            Vec::new(),
            0,
        );
    }

    let rect = area;
    let lines = lines.to_vec();
    let start = scroll.min(lines.len().saturating_sub(rect.height as usize));

    TextSurface::new(
        crate::selection::SelectablePane::SidebarDetail,
        rect,
        lines,
        start,
    )
}

pub fn thinking_detail_render_data(
    thinking: &str,
    theme: &Theme,
    content_w: usize,
    word_wrap: bool,
) -> SidebarDetailRenderData {
    let header = Line::from(vec![
        Span::styled("╭─", theme.muted_style()),
        Span::styled(
            " thinking trace ",
            theme.accent_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled("─╮", theme.muted_style()),
    ]);
    let body: Vec<Line<'static>> = if thinking.trim().is_empty() {
        vec![Line::from(Span::styled(
            "No streamed thinking trace",
            theme.muted_style(),
        ))]
    } else {
        thinking
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), theme.muted_style())))
            .collect()
    };
    let mut lines = vec![header];
    if word_wrap && content_w > 0 {
        lines.extend(wrap_styled_lines(&body, content_w.saturating_sub(2)));
    } else {
        lines.extend(body);
    }
    let plain_lines = lines.iter().map(line_to_plain_text).collect();
    SidebarDetailRenderData { lines, plain_lines }
}

pub fn build_detail_text_surface(
    tc: Option<&DisplayToolCall>,
    area: Rect,
    scroll: usize,
    ui_config: &UiConfig,
    highlighter: &Highlighter,
    theme: &Theme,
) -> TextSurface {
    if area.height == 0 || area.width == 0 {
        return TextSurface::new(
            crate::selection::SelectablePane::SidebarDetail,
            area,
            Vec::new(),
            0,
        );
    }

    let render = build_detail_render_data(tc, ui_config, highlighter, theme, area.width as usize);
    build_detail_text_surface_from_plain_lines(&render.plain_lines, area, scroll)
}

pub fn render_detail_from_lines(
    lines: &[Line<'_>],
    theme: &Theme,
    scroll: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    render_scrolled_lines(lines, area, buf, scroll);
    render_scroll_position_indicator(lines, theme, area, buf, scroll);
}

fn render_detail(
    tc: Option<&DisplayToolCall>,
    theme: &Theme,
    highlighter: &Highlighter,
    scroll: usize,
    ui_config: &UiConfig,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let Some(tc) = tc else {
        let lines = vec![Line::from(Span::styled(
            "Select a tool call",
            theme.muted_style(),
        ))];
        render_detail_from_lines(&lines, theme, scroll, area, buf);
        return;
    };

    let lines = styled_detail_lines(Some(tc), ui_config, highlighter, theme, area.width as usize);
    render_detail_from_lines(&lines, theme, scroll, area, buf);
}

fn styled_detail_lines(
    tc: Option<&DisplayToolCall>,
    ui_config: &UiConfig,
    highlighter: &Highlighter,
    theme: &Theme,
    content_w: usize,
) -> Vec<Line<'static>> {
    let Some(tc) = tc else {
        return vec![Line::from(Span::styled(
            "Select a tool call",
            theme.muted_style(),
        ))];
    };

    let full_config = UiConfig {
        tool_output: ToolOutputDisplay::Full,
        word_wrap: ui_config.word_wrap,
        ..*ui_config
    };
    let mut lines = Vec::new();
    if !uses_tool_card_detail(&tc.name) {
        lines.push(tc.header_line_animated_focused(theme, 0, true, ui_config.animations));
        let input_lines = tool_input_detail_lines(tc, theme, content_w.saturating_sub(2));
        lines.extend(input_lines);
    }
    lines.extend(styled_output_lines(
        tc,
        &full_config,
        highlighter,
        theme,
        content_w.saturating_sub(2),
    ));
    lines
}

fn uses_tool_card_detail(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "work"
            | "prototype"
            | "bash"
            | "shell"
            | "git"
            | "scan"
            | "web"
            | "read"
            | "write"
            | "edit"
            | "multi_edit"
    )
}

fn tool_input_detail_lines(
    tc: &DisplayToolCall,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let rows = tool_input_summary_rows(tc);
    if rows.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![Line::from(Span::styled("input", theme.muted_style()))];
    lines.extend(wrap_plain_lines(
        rows,
        width,
        &UiConfig {
            tool_output: ToolOutputDisplay::Full,
            word_wrap: true,
            ..Default::default()
        },
        theme,
        false,
    ));
    lines
}

fn tool_input_summary_rows(tc: &DisplayToolCall) -> Vec<String> {
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

fn styled_output_lines(
    tc: &DisplayToolCall,
    config: &UiConfig,
    highlighter: &Highlighter,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    if matches!(config.tool_output, ToolOutputDisplay::Collapsed) {
        return Vec::new();
    }

    if tc.name == "workflow" {
        let raw_lines = format_workflow_output(tc);
        let limited = apply_tool_output_limit(raw_lines, config);
        return wrap_plain_lines(limited, width, config, theme, tc.is_error);
    }

    if tc.output.is_none() && !tc.streaming_output.is_empty() {
        let live_lines = tc
            .streaming_output
            .lines()
            .map(String::from)
            .collect::<Vec<_>>();
        let limited = apply_tool_output_limit(live_lines, config);
        return wrap_plain_lines(limited, width, config, theme, tc.is_error);
    }

    if tc.output.is_none() && !tc.streaming_lines.is_empty() {
        let limited = apply_tool_output_limit(tc.streaming_lines.clone(), config);
        return wrap_plain_lines(limited, width, config, theme, tc.is_error);
    }

    if tc.output.is_none() {
        return wrap_plain_lines(
            vec!["Running…".to_string()],
            width,
            config,
            theme,
            tc.is_error,
        );
    }

    let styled = styled_sidebar_tool_output_lines(tc, highlighter, theme, tc.name == "read");
    let styled = apply_styled_tool_output_limit(styled, config, theme);
    if config.word_wrap && width > 0 {
        wrap_styled_lines(&styled, width.saturating_sub(2))
    } else {
        styled
    }
}

fn apply_tool_output_limit(raw_lines: Vec<String>, config: &UiConfig) -> Vec<String> {
    match config.tool_output {
        ToolOutputDisplay::Compact => {
            let max = config.tool_output_lines;
            if raw_lines.len() > max {
                let mut out: Vec<String> = raw_lines.into_iter().take(max).collect();
                out.push("…".to_string());
                out
            } else {
                raw_lines
            }
        }
        _ => raw_lines,
    }
}

fn apply_styled_tool_output_limit(
    lines: Vec<Line<'static>>,
    config: &UiConfig,
    theme: &Theme,
) -> Vec<Line<'static>> {
    match config.tool_output {
        ToolOutputDisplay::Compact => {
            let max = config.tool_output_lines;
            if lines.len() > max {
                let mut out: Vec<Line<'static>> = lines.into_iter().take(max).collect();
                out.push(Line::from(Span::styled("…", theme.muted_style())));
                out
            } else {
                lines
            }
        }
        _ => lines,
    }
}

fn wrap_plain_lines(
    lines: Vec<String>,
    width: usize,
    config: &UiConfig,
    theme: &Theme,
    is_error: bool,
) -> Vec<Line<'static>> {
    let style = if is_error {
        theme.error_style()
    } else {
        theme.muted_style()
    };

    let lines: Vec<Line<'static>> = lines
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect();

    if config.word_wrap && width > 0 {
        wrap_styled_lines(&lines, width.saturating_sub(2))
    } else {
        lines
    }
}

fn indent_line(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw("  ".to_string())];
    spans.extend(line.spans);
    Line::from(spans)
}

fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
fn format_workflow_output(tc: &DisplayToolCall) -> Vec<String> {
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

#[cfg(test)]
fn wrap_into(line: &str, width: usize, out: &mut Vec<String>) {
    if width == 0 {
        out.push(String::new());
        return;
    }

    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= width {
        out.push(line.to_string());
        return;
    }

    let mut start = 0;
    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= width {
            out.push(chars[start..].iter().collect());
            break;
        }

        let end = start + width;
        if end >= chars.len() || chars[end] == ' ' {
            let segment: String = chars[start..end].iter().collect();
            out.push(segment);
            start = if end < chars.len() { end + 1 } else { end };
            continue;
        }

        let mut break_at = None;
        for i in (start + 1..end).rev() {
            if chars[i] == ' ' {
                break_at = Some(i);
                break;
            }
        }

        if let Some(bp) = break_at {
            let segment: String = chars[start..bp].iter().collect();
            out.push(segment);
            start = bp + 1;
        } else {
            let segment: String = chars[start..end].iter().collect();
            out.push(segment);
            start = end;
        }
    }
}

#[cfg(test)]
#[path = "sidebar/tests.rs"]
mod tests;
