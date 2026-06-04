use std::collections::HashMap;
use std::time::Duration;

use crate::animation::{
    activity_label, format_elapsed, queued_glyph, ActivitySurface, AnimationState,
};
use imp_core::config::AnimationLevel;
use imp_llm::ThinkingLevel;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowMode {
    Normal,
    Improve,
}

impl WorkflowMode {
    pub fn label(self) -> &'static str {
        match self {
            WorkflowMode::Normal => "",
            WorkflowMode::Improve => "IMPROVE",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            WorkflowMode::Normal => "Normal",
            WorkflowMode::Improve => "Improve",
        }
    }
}

/// Multi-line editor state with cursor management.
#[derive(Debug, Clone)]
pub struct EditorState {
    pub content: String,
    pub cursor: usize,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub scroll_offset: usize,
    paste_ranges: Vec<std::ops::Range<usize>>,
}

impl EditorState {
    fn normalized_cursor(&self) -> usize {
        clamp_cursor_to_boundary(&self.content, self.cursor)
    }

    fn normalize_cursor(&mut self) {
        self.cursor = self.normalized_cursor();
    }

    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor: 0,
            cursor_line: 0,
            cursor_col: 0,
            history: Vec::new(),
            history_idx: None,
            scroll_offset: 0,
            paste_ranges: Vec::new(),
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.normalize_cursor();
        let at = self.cursor;
        self.content.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.record_insert(at, c.len_utf8());
        self.update_position();
    }

    pub fn insert_newline(&mut self) {
        self.normalize_cursor();
        let at = self.cursor;
        self.content.insert(self.cursor, '\n');
        self.cursor += 1;
        self.record_insert(at, 1);
        self.update_position();
    }

    pub fn insert_paste(&mut self, text: &str) {
        self.normalize_cursor();
        let start = self.cursor;
        self.content.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.record_insert(start, text.len());
        if crate::views::chat::pasted_block_summary(text).is_some() {
            self.paste_ranges.push(start..self.cursor);
        }
        self.update_position();
    }

    pub fn delete_back(&mut self) {
        self.normalize_cursor();
        if self.cursor > 0 {
            let prev = prev_char_boundary(&self.content, self.cursor);
            self.content.drain(prev..self.cursor);
            self.record_delete(prev..self.cursor);
            self.cursor = prev;
            self.update_position();
        }
    }

    pub fn delete_forward(&mut self) {
        self.normalize_cursor();
        if self.cursor < self.content.len() {
            let next = next_char_boundary(&self.content, self.cursor);
            self.content.drain(self.cursor..next);
            self.record_delete(self.cursor..next);
            self.update_position();
        }
    }

    pub fn move_left(&mut self) {
        self.normalize_cursor();
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.content, self.cursor);
            self.update_position();
        }
    }

    pub fn move_right(&mut self) {
        self.normalize_cursor();
        if self.cursor < self.content.len() {
            self.cursor = next_char_boundary(&self.content, self.cursor);
            self.update_position();
        }
    }

    pub fn move_up(&mut self) -> bool {
        self.normalize_cursor();
        self.update_position();
        if self.cursor_line == 0 {
            return false; // signal: at top, caller may use for history
        }
        let lines: Vec<&str> = self.content.split('\n').collect();
        let target_line = self.cursor_line - 1;
        let target_col = self.cursor_col.min(lines[target_line].len());
        self.cursor = line_col_to_byte(&lines, target_line, target_col);
        self.update_position();
        true
    }

    pub fn move_down(&mut self) -> bool {
        self.normalize_cursor();
        self.update_position();
        let lines: Vec<&str> = self.content.split('\n').collect();
        if self.cursor_line >= lines.len() - 1 {
            return false; // signal: at bottom, caller may use for history
        }
        let target_line = self.cursor_line + 1;
        let target_col = self.cursor_col.min(lines[target_line].len());
        self.cursor = line_col_to_byte(&lines, target_line, target_col);
        self.update_position();
        true
    }

    pub fn move_home(&mut self) {
        self.normalize_cursor();
        let before = &self.content[..self.cursor];
        self.cursor = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
        self.update_position();
    }

    pub fn move_end(&mut self) {
        self.normalize_cursor();
        let after = &self.content[self.cursor..];
        self.cursor += after.find('\n').unwrap_or(after.len());
        self.update_position();
    }

    pub fn move_word_left(&mut self) {
        self.normalize_cursor();
        if self.cursor == 0 {
            return;
        }
        let bytes = self.content.as_bytes();
        let mut pos = self.cursor;
        // Skip whitespace
        while pos > 0 && bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        // Skip word chars
        while pos > 0 && !bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }
        self.cursor = pos;
        self.update_position();
    }

    pub fn move_word_right(&mut self) {
        self.normalize_cursor();
        let bytes = self.content.as_bytes();
        let len = bytes.len();
        let mut pos = self.cursor;
        // Skip current word
        while pos < len && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        // Skip whitespace
        while pos < len && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        self.cursor = pos;
        self.update_position();
    }

    pub fn delete_word_back(&mut self) {
        self.normalize_cursor();
        if self.cursor == 0 {
            return;
        }
        let start = self.cursor;
        self.move_word_left();
        self.content.drain(self.cursor..start);
        self.record_delete(self.cursor..start);
        self.update_position();
    }

    pub fn delete_to_start(&mut self) {
        self.normalize_cursor();
        let line_start = {
            let before = &self.content[..self.cursor];
            before.rfind('\n').map(|p| p + 1).unwrap_or(0)
        };
        self.content.drain(line_start..self.cursor);
        self.record_delete(line_start..self.cursor);
        self.cursor = line_start;
        self.update_position();
    }

    pub fn delete_to_end(&mut self) {
        self.normalize_cursor();
        let line_end = {
            let after = &self.content[self.cursor..];
            self.cursor + after.find('\n').unwrap_or(after.len())
        };
        self.content.drain(self.cursor..line_end);
        self.record_delete(self.cursor..line_end);
        self.update_position();
    }

    pub fn clear(&mut self) {
        self.content.clear();
        self.paste_ranges.clear();
        self.cursor = 0;
        self.update_position();
    }

    pub fn set_content(&mut self, text: &str) {
        self.content = text.to_string();
        self.paste_ranges.clear();
        self.cursor = self.content.len();
        self.update_position();
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    fn record_insert(&mut self, at: usize, len: usize) {
        for range in &mut self.paste_ranges {
            if range.start >= at {
                range.start += len;
                range.end += len;
            } else if range.end > at {
                range.end += len;
            }
        }
    }

    fn record_delete(&mut self, deleted: std::ops::Range<usize>) {
        let len = deleted.end.saturating_sub(deleted.start);
        self.paste_ranges.retain_mut(|range| {
            let overlaps = range.start < deleted.end && range.end > deleted.start;
            if overlaps {
                return false;
            }
            if range.start >= deleted.end {
                range.start = range.start.saturating_sub(len);
                range.end = range.end.saturating_sub(len);
            }
            true
        });
    }

    pub fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.content.split('\n').count().max(1)
    }

    pub fn visual_line_count_with_summary(&self, inner_width: u16, summarize_paste: bool) -> usize {
        editor_display_lines(
            &self.content,
            &self.paste_ranges,
            inner_width,
            summarize_paste,
        )
        .len()
        .max(1)
    }

    pub fn visual_line_count(&self, inner_width: u16) -> usize {
        self.visual_line_count_with_summary(inner_width, false)
    }

    pub fn push_history(&mut self) {
        if !self.content.trim().is_empty() {
            self.history.push(self.content.clone());
        }
        self.history_idx = None;
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            Some(i) if i > 0 => i - 1,
            Some(_) => return,
            None => {
                if !self.content.is_empty() {
                    self.history.push(self.content.clone());
                }
                self.history.len() - 1
            }
        };
        self.history_idx = Some(idx);
        self.content = self.history[idx].clone();
        self.cursor = self.content.len();
        self.update_position();
    }

    pub fn history_next(&mut self) {
        if let Some(i) = self.history_idx {
            if i + 1 < self.history.len() {
                self.history_idx = Some(i + 1);
                self.content = self.history[i + 1].clone();
            } else {
                self.history_idx = None;
                self.content.clear();
            }
            self.cursor = self.content.len();
            self.update_position();
        }
    }

    /// Calculate cursor position relative to a render area, accounting for soft wraps.
    pub fn cursor_screen_position(&self, area: Rect) -> (u16, u16) {
        if area.width == 0 || area.height == 0 {
            return (area.x, area.y);
        }

        let inner_x = area.x.saturating_add(1); // account for border
        let inner_y = area.y.saturating_add(1);
        let inner_width = area.width.saturating_sub(2).max(1);
        let cursor = self.normalized_cursor();
        let (visual_line, visual_col) =
            cursor_visual_position_for_text(&self.content, cursor, inner_width);
        let x = inner_x.saturating_add(visual_col as u16);
        let y =
            inner_y.saturating_add((visual_line as u16).saturating_sub(self.scroll_offset as u16));
        let max_x = area.x.saturating_add(area.width.saturating_sub(2));
        let max_y = area.y.saturating_add(area.height.saturating_sub(2));
        (x.min(max_x), y.min(max_y))
    }

    fn update_position(&mut self) {
        self.normalize_cursor();
        let before = &self.content[..self.cursor];
        self.cursor_line = before.matches('\n').count();
        self.cursor_col = before
            .rfind('\n')
            .map(|p| self.cursor - p - 1)
            .unwrap_or(self.cursor);
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

/// The editor widget renders the input area with border and cursor.
pub struct EditorView<'a> {
    state: &'a EditorState,
    theme: &'a Theme,
    thinking_level: ThinkingLevel,
    summarize_paste: bool,
    model_name: &'a str,
    cwd: &'a str,
    session_name: &'a str,
    is_streaming: bool,
    queued_preview: Option<String>,
    current_context_tokens: u32,
    context_window: u32,
    show_context_usage: bool,
    turn_elapsed: Option<Duration>,
    extension_items: Option<&'a HashMap<String, String>>,
    peek: bool,
    tick: u64,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
    _workflow_mode: WorkflowMode,
    workflow_scope_label: Option<String>,
    workflow_run_label: Option<String>,
    build_loop_label: Option<String>,
    improve_status_label: Option<String>,
    loop_label: Option<String>,
    git_label: Option<String>,
}

impl<'a> EditorView<'a> {
    pub fn new(state: &'a EditorState, theme: &'a Theme, thinking_level: ThinkingLevel) -> Self {
        Self {
            state,
            theme,
            thinking_level,
            summarize_paste: false,
            model_name: "",
            cwd: "",
            session_name: "",
            is_streaming: false,
            queued_preview: None,
            current_context_tokens: 0,
            context_window: 0,
            show_context_usage: true,
            turn_elapsed: None,
            extension_items: None,
            peek: false,
            tick: 0,
            animation_level: AnimationLevel::Minimal,
            activity_state: AnimationState::Idle,
            _workflow_mode: WorkflowMode::Normal,
            workflow_scope_label: None,
            workflow_run_label: None,
            build_loop_label: None,
            improve_status_label: None,
            loop_label: None,
            git_label: None,
        }
    }

    pub fn summarize_paste(mut self, summarize: bool) -> Self {
        self.summarize_paste = summarize;
        self
    }

    /// Set the model name shown in the editor border.
    pub fn model(mut self, name: &'a str) -> Self {
        self.model_name = name;
        self
    }

    pub fn identity(mut self, cwd: &'a str, session_name: &'a str) -> Self {
        self.cwd = cwd;
        self.session_name = session_name;
        self
    }

    pub fn turn_elapsed(mut self, elapsed: Option<Duration>) -> Self {
        self.turn_elapsed = elapsed;
        self
    }

    pub fn extension_items(mut self, items: &'a HashMap<String, String>, peek: bool) -> Self {
        self.extension_items = Some(items);
        self.peek = peek;
        self
    }

    pub fn streaming(mut self, streaming: bool) -> Self {
        self.is_streaming = streaming;
        self
    }

    pub fn queued(mut self, preview: Option<String>) -> Self {
        self.queued_preview = preview;
        self
    }

    pub fn context_usage(mut self, current_tokens: u32, context_window: u32, show: bool) -> Self {
        self.current_context_tokens = current_tokens;
        self.context_window = context_window;
        self.show_context_usage = show;
        self
    }

    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    pub fn animation_level(mut self, level: AnimationLevel) -> Self {
        self.animation_level = level;
        self
    }

    pub fn activity_state(mut self, state: AnimationState) -> Self {
        self.activity_state = state;
        self
    }

    pub fn workflow_mode(mut self, mode: WorkflowMode) -> Self {
        self._workflow_mode = mode;
        self
    }

    pub fn workflow_scope_label(mut self, label: Option<String>) -> Self {
        self.workflow_scope_label = label;
        self
    }

    pub fn workflow_run_label(mut self, label: Option<String>) -> Self {
        self.workflow_run_label = label;
        self
    }

    pub fn build_loop_label(mut self, label: Option<String>) -> Self {
        self.build_loop_label = label;
        self
    }

    pub fn improve_status_label(mut self, label: Option<String>) -> Self {
        self.improve_status_label = label;
        self
    }

    pub fn loop_label(mut self, label: Option<String>) -> Self {
        self.loop_label = label;
        self
    }

    pub fn git_label(mut self, label: Option<String>) -> Self {
        self.git_label = label;
        self
    }
}

impl Widget for EditorView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 4 {
            return;
        }

        let prompt_activity_state = if self.queued_preview.is_some() {
            AnimationState::Queued
        } else {
            self.activity_state
        };

        let border_style = superbar_border_style(self.theme, self.thinking_level);

        let top_left = build_identity_label(self.cwd, self.session_name, area.width);
        let top_right = build_top_right_label(self.turn_elapsed, self.theme);
        let bottom_left = build_bottom_left_label(
            self._workflow_mode,
            self.workflow_scope_label.as_deref(),
            self.workflow_run_label.as_deref(),
            self.build_loop_label.as_deref(),
        );
        let activity =
            editor_activity_label(prompt_activity_state, self.tick, self.animation_level);

        // Build bottom-right metadata cluster
        let thinking_label = match self.thinking_level {
            ThinkingLevel::Off => "",
            ThinkingLevel::Minimal => "min",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "med",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
        };
        let model_label = if self.model_name.is_empty() {
            None
        } else {
            Some(self.model_name.to_string())
        };
        let queue_label = None;
        let context_ratio = if self.context_window > 0 {
            self.current_context_tokens as f64 / self.context_window as f64
        } else {
            0.0
        };
        let context_style = if context_ratio >= 0.75 {
            self.theme.error_style()
        } else if context_ratio >= 0.50 {
            self.theme.warning_style()
        } else {
            self.theme.muted_style()
        };
        let mut bottom_spans = Vec::new();
        let mut push_part = |text: String, style: Style| {
            if !bottom_spans.is_empty() {
                bottom_spans.push(Span::styled(" · ".to_string(), self.theme.muted_style()));
            }
            bottom_spans.push(Span::styled(text, style));
        };
        if let Some(model) = model_label {
            push_part(model, self.theme.accent_style());
        }
        if !thinking_label.is_empty() {
            push_part(
                thinking_label.to_string(),
                Style::default().fg(self.theme.thinking_border_color(self.thinking_level)),
            );
        }
        if self.show_context_usage && self.context_window > 0 {
            push_part(
                format_context_usage(self.current_context_tokens, self.context_window),
                context_style,
            );
        }
        if let Some(git) = self.git_label.as_deref() {
            push_part(git.to_string(), self.theme.muted_style());
        }
        if let Some(queue) = queue_label {
            push_part(queue, self.theme.warning_style());
        }
        if let Some(loop_label) = self.loop_label.as_deref() {
            push_part(loop_label.to_string(), self.theme.warning_style());
        }
        if !activity.is_empty() {
            push_part(activity, self.theme.muted_style());
        }

        let block = Block::default()
            .title(Line::from(top_left))
            .title(Line::from(top_right).alignment(Alignment::Right))
            .title_bottom(Line::from(bottom_left))
            .title_bottom(Line::from(bottom_spans).alignment(Alignment::Right))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        let mut content_inner = inner;
        if let Some(status) = self.improve_status_label.as_deref() {
            if inner.height > 1 {
                let status_y = content_inner.y;
                buf.set_line(
                    content_inner.x,
                    status_y,
                    &Line::from(Span::styled(status.to_string(), self.theme.accent_style())),
                    content_inner.width,
                );
                content_inner.y = content_inner.y.saturating_add(1);
                content_inner.height = content_inner.height.saturating_sub(1);
            }
        }
        if let Some(preview) = self.queued_preview.as_deref() {
            if inner.height > 1 {
                let queue_y = inner.y + inner.height - 1;
                let label = format!("{} queued {}", queued_glyph(), preview);
                buf.set_line(
                    inner.x,
                    queue_y,
                    &Line::from(Span::styled(label, self.theme.warning_style())),
                    inner.width,
                );
                content_inner.height = content_inner.height.saturating_sub(1);
            }
        }

        // Render editor content using wrapped visual lines so auto-grow and cursor math stay aligned.
        let lines = editor_display_lines(
            &self.state.content,
            &self.state.paste_ranges,
            content_inner.width,
            self.summarize_paste,
        )
        .into_iter()
        .skip(self.state.scroll_offset)
        .take(content_inner.height as usize)
        .collect::<Vec<_>>();

        for (idx, line) in lines.iter().enumerate() {
            if idx >= content_inner.height as usize {
                break;
            }
            buf.set_line(
                content_inner.x,
                content_inner.y + idx as u16,
                &Line::raw(line.clone()),
                content_inner.width,
            );
        }

        // Placeholder text when empty and not streaming
        if self.state.content.is_empty() && !self.is_streaming && content_inner.height > 0 {
            let placeholder = "Ask imp anything…  @file attach · / commands · ! shell";
            buf.set_string(
                content_inner.x,
                content_inner.y,
                placeholder,
                Style::default().fg(Color::DarkGray),
            );
        }
    }
}

// --- Helpers ---

fn editor_display_lines(
    text: &str,
    paste_ranges: &[std::ops::Range<usize>],
    inner_width: u16,
    summarize_paste: bool,
) -> Vec<String> {
    if !summarize_paste || paste_ranges.is_empty() {
        return wrapped_lines_for_width(text, inner_width);
    }

    let mut display = String::new();
    let mut cursor = 0usize;
    let mut ranges = paste_ranges
        .iter()
        .filter(|range| {
            range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
        })
        .cloned()
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    for range in ranges {
        if range.start < cursor {
            continue;
        }
        display.push_str(&text[cursor..range.start]);
        let pasted = &text[range.clone()];
        if let Some(summary) = pasted_inline_summary(pasted) {
            display.push_str(&summary);
        } else {
            display.push_str(pasted);
        }
        cursor = range.end;
    }
    display.push_str(&text[cursor..]);

    wrapped_lines_for_width(&display, inner_width)
}

fn pasted_inline_summary(text: &str) -> Option<String> {
    crate::views::chat::pasted_block_summary(text)?;
    let first = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let preview = truncate_display_width(first, 48);
    let extra_lines = text.lines().count().saturating_sub(1);
    Some(format!("[{preview} + {extra_lines} lines]"))
}

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }

    let suffix = "…";
    let target = max_width.saturating_sub(display_width(suffix));
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        if width + ch_width > target {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str(suffix);
    out
}

fn build_identity_label(cwd: &str, session_name: &str, area_width: u16) -> Vec<Span<'static>> {
    let max_path = (area_width as usize / 3).clamp(12, 36);
    let cwd = abbreviate_home(cwd);
    let cwd = shorten_path(&cwd, max_path);
    let session_name = session_name.trim();

    let mut spans = vec![Span::raw(cwd)];
    if !session_name.is_empty() {
        spans.push(Span::raw(" · "));
        spans.push(Span::raw(session_name.to_string()));
    }
    spans
}

fn build_top_right_label(turn_elapsed: Option<Duration>, theme: &Theme) -> Vec<Span<'static>> {
    turn_elapsed
        .map(|elapsed| vec![Span::styled(format_elapsed(elapsed), theme.muted_style())])
        .unwrap_or_default()
}

fn build_bottom_left_label(
    _workflow_mode: WorkflowMode,
    workflow_scope_label: Option<&str>,
    workflow_run_label: Option<&str>,
    build_loop_label: Option<&str>,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(scope) = workflow_scope_label.filter(|scope| !scope.trim().is_empty()) {
        spans.push(Span::raw(scope.to_string()));
    }
    if let Some(run) = workflow_run_label.filter(|label| !label.trim().is_empty()) {
        spans.push(Span::raw(" · "));
        spans.push(Span::raw(run.to_string()));
    }
    if let Some(loop_state) = build_loop_label.filter(|label| !label.trim().is_empty()) {
        spans.push(Span::raw(" · "));
        spans.push(Span::raw(loop_state.to_string()));
    }
    spans
}

fn editor_activity_label(
    activity_state: AnimationState,
    tick: u64,
    animation_level: AnimationLevel,
) -> String {
    match activity_state {
        AnimationState::Thinking | AnimationState::WaitingForResponse => String::new(),
        _ => activity_label(
            activity_state,
            tick,
            animation_level,
            ActivitySurface::Editor,
        ),
    }
}

fn superbar_border_style(theme: &Theme, thinking_level: ThinkingLevel) -> Style {
    Style::default().fg(theme.thinking_border_color(thinking_level))
}

fn abbreviate_home(path: &str) -> String {
    if path == "/Users/asher" {
        "~".to_string()
    } else if let Some(rest) = path.strip_prefix("/Users/asher/") {
        format!("~/{rest}")
    } else {
        path.to_string()
    }
}

fn shorten_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        let shortened = shorten_path(&format!("home/{rest}"), max_len.saturating_sub(1));
        return shortened.replacen("home/", "~/", 1);
    }

    let parts: Vec<&str> = path.split('/').collect();
    let mut result = String::new();
    for part in parts.iter().rev() {
        let candidate = if result.is_empty() {
            part.to_string()
        } else {
            format!("{part}/{result}")
        };
        if candidate.len() > max_len {
            break;
        }
        result = candidate;
    }

    if result.len() < path.len() {
        format!("…/{result}")
    } else {
        result
    }
}

fn format_context_usage(current_tokens: u32, context_window: u32) -> String {
    if context_window == 0 {
        return format_compact_tokens(current_tokens);
    }
    let percent = ((current_tokens as f64 / context_window as f64) * 100.0).round();
    format!("{percent:.0}%/{}", format_compact_tokens(context_window))
}

fn format_compact_tokens(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        let value = tokens as f64 / 1_000.0;
        if value >= 100.0 {
            format!("{:.0}k", value)
        } else if value >= 10.0 {
            format!("{:.1}k", value)
        } else {
            format!("{:.2}k", value)
        }
    } else {
        tokens.to_string()
    }
}

fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos;
    while p > 0 {
        p -= 1;
        if s.is_char_boundary(p) {
            return p;
        }
    }
    0
}

fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p < s.len() {
        p += 1;
        if s.is_char_boundary(p) {
            return p;
        }
    }
    s.len()
}

pub fn clamp_cursor_to_boundary(text: &str, cursor: usize) -> usize {
    let mut clamped = cursor.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

fn line_col_to_byte(lines: &[&str], line: usize, col: usize) -> usize {
    let mut byte = 0;
    for (i, l) in lines.iter().enumerate() {
        if i == line {
            return byte + col.min(l.len());
        }
        byte += l.len() + 1; // +1 for \n
    }
    byte
}

pub fn wrapped_lines_for_width(text: &str, inner_width: u16) -> Vec<String> {
    let width = inner_width.max(1) as usize;
    let mut out = Vec::new();

    for logical in text.split('\n') {
        if logical.is_empty() {
            out.push(String::new());
            continue;
        }

        wrap_logical_line(logical, width, &mut out);
    }

    if out.is_empty() {
        out.push(String::new());
    }

    out
}

fn wrap_logical_line(logical: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut last_whitespace_byte = None;

    for ch in logical.chars() {
        let ch_width = char_display_width(ch);

        if !current.is_empty() && current_width + ch_width > width {
            if let Some(split_byte) = last_whitespace_byte {
                let next = current[split_byte..].trim_start().to_string();
                let line = current[..split_byte].trim_end().to_string();

                if !line.is_empty() {
                    out.push(line);
                }

                current = next;
                current_width = display_width(&current);
                last_whitespace_byte = last_whitespace_byte_in(&current);
            } else {
                out.push(current);
                current = String::new();
                current_width = 0;
                last_whitespace_byte = None;
            }
        }

        if current.is_empty() && ch_width > width {
            out.push(ch.to_string());
            continue;
        }

        current.push(ch);
        current_width += ch_width;

        if ch.is_whitespace() {
            last_whitespace_byte = Some(current.len());
        }

        if current_width == width {
            if let Some(split_byte) = last_whitespace_byte {
                let next = current[split_byte..].trim_start().to_string();
                let line = current[..split_byte].trim_end().to_string();

                if !line.is_empty() {
                    out.push(line);
                }

                current = next;
                current_width = display_width(&current);
                last_whitespace_byte = last_whitespace_byte_in(&current);
            } else {
                out.push(current);
                current = String::new();
                current_width = 0;
                last_whitespace_byte = None;
            }
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn last_whitespace_byte_in(text: &str) -> Option<usize> {
    text.char_indices()
        .filter_map(|(idx, ch)| ch.is_whitespace().then_some(idx + ch.len_utf8()))
        .next_back()
}

pub fn cursor_visual_position_for_text(
    text: &str,
    cursor: usize,
    inner_width: u16,
) -> (usize, usize) {
    let cursor = clamp_cursor_to_boundary(text, cursor);
    let before_cursor = &text[..cursor];
    let lines = wrapped_lines_for_width(before_cursor, inner_width);
    let row = lines.len().saturating_sub(1);
    let col = lines.last().map(|line| display_width(line)).unwrap_or(0);

    (row, col)
}

fn char_display_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        _ => ch.width().unwrap_or(1).max(1),
    }
}

#[cfg(test)]
#[path = "editor/tests.rs"]
mod tests;
