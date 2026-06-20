use imp_core::config::{AnimationLevel, SidebarStyle, ToolOutputDisplay, UiConfig};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::highlight::Highlighter;
use crate::selection::TextSurface;
use crate::theme::Theme;
use crate::views::tool_output::{styled_sidebar_tool_output_lines, wrap_styled_lines};
use crate::views::tools::DisplayToolCall;

mod input;
mod workflow;

use input::tool_input_summary_rows;
use workflow::format_workflow_output;

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
