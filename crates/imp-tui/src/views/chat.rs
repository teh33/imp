use imp_core::config::{AnimationLevel, ChatToolDisplay};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::animation::{activity_label, ActivitySurface, AnimationState};
use crate::highlight::Highlighter;
use crate::markdown;
use crate::selection::TextSurface;
use crate::theme::Theme;
use crate::views::tool_output::styled_tool_output_lines;
use crate::views::tools::{tool_call_height, DisplayToolCall};

#[derive(Debug)]
pub struct ChatRenderData {
    pub lines: Vec<Line<'static>>,
    pub tool_line_indices: Vec<(usize, String)>,
}

/// Role of a display message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Warning,
    Compaction,
    Error,
}

/// Ordered display blocks inside an assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayAssistantBlock {
    Text(String),
    ThoughtDuration { seconds: u64 },
    ToolCall { id: String },
}

/// A message formatted for display in the chat view.
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<DisplayToolCall>,
    pub assistant_blocks: Vec<DisplayAssistantBlock>,
    pub is_streaming: bool,
    pub timestamp: u64,
}

impl DisplayMessage {
    /// Construct from an imp_llm Message.
    pub fn from_message(msg: &imp_llm::Message) -> Self {
        match msg {
            imp_llm::Message::User(u) => {
                let text = u
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                Self {
                    role: MessageRole::User,
                    content: text,
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: u.timestamp,
                }
            }
            imp_llm::Message::Assistant(a) => {
                let mut display = Self {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: a.timestamp,
                };
                for block in &a.content {
                    match block {
                        imp_llm::ContentBlock::Text { text: t } => {
                            display.add_assistant_text_block(t);
                        }
                        imp_llm::ContentBlock::Thinking { text: t } => {
                            match &mut display.thinking {
                                Some(existing) => existing.push_str(t),
                                None => display.thinking = Some(t.clone()),
                            }
                        }
                        imp_llm::ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            display.push_assistant_tool_call(DisplayToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                args_summary: DisplayToolCall::make_args_summary(name, arguments),
                                output: None,
                                details: arguments.clone(),
                                is_error: false,
                                expanded: false,
                                notices: Vec::new(),
                                streaming_lines: Vec::new(),
                                streaming_output: String::new(),
                            });
                        }
                        _ => {}
                    }
                }
                display
            }
            imp_llm::Message::ToolResult(t) => {
                let text = t
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                Self {
                    role: if t.is_error {
                        MessageRole::Error
                    } else {
                        MessageRole::System
                    },
                    content: text,
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: t.timestamp,
                }
            }
        }
    }

    pub fn add_assistant_text_block(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.content.push_str(text);
        if let Some(DisplayAssistantBlock::Text(existing)) = self.assistant_blocks.last_mut() {
            existing.push_str(text);
        } else {
            self.assistant_blocks
                .push(DisplayAssistantBlock::Text(text.to_string()));
        }
    }

    pub fn push_assistant_text_delta(&mut self, text: &str) {
        self.add_assistant_text_block(text);
    }

    pub fn push_assistant_thought_duration(&mut self, seconds: u64) {
        let seconds = seconds.max(1);
        if matches!(
            self.assistant_blocks.last(),
            Some(DisplayAssistantBlock::ThoughtDuration { .. })
        ) {
            return;
        }
        self.assistant_blocks
            .push(DisplayAssistantBlock::ThoughtDuration { seconds });
    }

    pub fn push_assistant_tool_call(&mut self, tool_call: DisplayToolCall) {
        let id = tool_call.id.clone();
        self.tool_calls.push(tool_call);
        self.assistant_blocks
            .push(DisplayAssistantBlock::ToolCall { id });
    }

    fn find_tool_call(&self, id: &str) -> Option<&DisplayToolCall> {
        self.tool_calls.iter().find(|tc| tc.id == id)
    }

    /// Calculate the rendered line count for this message.
    pub fn line_count(&self, theme: &Theme, highlighter: &Highlighter) -> usize {
        let mut count = 0;

        // Prefix line
        count += 1;

        // Content lines (markdown renders to lines)
        if !self.content.is_empty() {
            match self.role {
                MessageRole::Assistant => {
                    count += markdown::render_markdown(&self.content, theme, highlighter).len();
                }
                _ => {
                    count += self.content.lines().count().max(1);
                }
            }
        }

        // Thinking block
        if self.thinking.is_some() {
            count += 1; // header
        }

        // Tool calls
        for tc in &self.tool_calls {
            count += tool_call_height(tc) as usize;
        }

        // Separator
        count += 1;
        count
    }
}

const PASTED_SUMMARY_MIN_LINES: usize = 3;
const PASTED_SUMMARY_MIN_CODE_LIKE_LINES: usize = 3;

pub fn summarize_user_text_for_display(text: &str) -> String {
    pasted_block_summary(text).unwrap_or_else(|| text.to_string())
}

pub fn pasted_block_summary(text: &str) -> Option<String> {
    let line_count = text.lines().count();
    if line_count < PASTED_SUMMARY_MIN_LINES {
        return None;
    }

    let code_like_lines = text.lines().filter(|line| is_code_like_line(line)).count();
    if code_like_lines < PASTED_SUMMARY_MIN_CODE_LIKE_LINES {
        return None;
    }

    Some(format!(
        "[Pasted {line_count} {}]",
        if line_count == 1 { "Line" } else { "Lines" }
    ))
}

fn is_code_like_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("```") {
        return true;
    }

    if line.starts_with(' ') || line.starts_with('\t') {
        return true;
    }

    if trimmed.ends_with('{')
        || trimmed.ends_with('}')
        || trimmed.ends_with(';')
        || trimmed.ends_with(",")
        || trimmed.ends_with(")")
        || trimmed.ends_with("]")
    {
        return true;
    }

    [
        "fn ",
        "let ",
        "const ",
        "pub ",
        "impl ",
        "use ",
        "mod ",
        "struct ",
        "enum ",
        "trait ",
        "async ",
        "await ",
        "return ",
        "if ",
        "else",
        "match ",
        "for ",
        "while ",
        "loop ",
        "class ",
        "def ",
        "import ",
        "from ",
        "function ",
        "interface ",
        "type ",
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "CREATE ",
        "ALTER ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
        || trimmed.contains("::")
        || trimmed.contains("->")
        || trimmed.contains("=>")
        || trimmed.contains("</")
        || trimmed.contains("/>")
}

/// Chat view: displays conversation messages with streaming support.
pub struct ChatView<'a> {
    messages: &'a [DisplayMessage],
    theme: &'a Theme,
    highlighter: &'a Highlighter,
    precomputed_lines: Option<&'a [Line<'static>]>,
    scroll_offset: usize,
    tick: u64,
    /// Flat index of the focused tool call across all messages, if any.
    tool_focus: Option<usize>,
    /// Word-wrap long chat lines to the current viewport width.
    word_wrap: bool,
    /// How tool calls should appear in the chat transcript.
    chat_tool_display: ChatToolDisplay,
    /// Maximum rendered rows for expanded inline shell output.
    tool_output_lines: usize,
    /// Number of thinking lines to show.
    thinking_lines: usize,
    /// Whether to show timestamps above messages.
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
}

impl<'a> ChatView<'a> {
    pub fn new(
        messages: &'a [DisplayMessage],
        theme: &'a Theme,
        highlighter: &'a Highlighter,
    ) -> Self {
        Self {
            messages,
            theme,
            highlighter,
            precomputed_lines: None,
            scroll_offset: 0,
            tick: 0,
            tool_focus: None,
            word_wrap: true,
            chat_tool_display: ChatToolDisplay::Interleaved,
            tool_output_lines: 10,
            thinking_lines: 5,
            show_timestamps: false,
            animation_level: AnimationLevel::Minimal,
            activity_state: AnimationState::Idle,
        }
    }

    pub fn precomputed_lines(mut self, lines: &'a [Line<'static>]) -> Self {
        self.precomputed_lines = Some(lines);
        self
    }

    pub fn scroll(mut self, offset: usize) -> Self {
        self.scroll_offset = offset;
        self
    }

    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    pub fn tool_focus(mut self, focus: Option<usize>) -> Self {
        self.tool_focus = focus;
        self
    }

    pub fn word_wrap(mut self, enabled: bool) -> Self {
        self.word_wrap = enabled;
        self
    }

    pub fn chat_tool_display(mut self, display: ChatToolDisplay) -> Self {
        self.chat_tool_display = display;
        self
    }

    pub fn tool_output_lines(mut self, lines: usize) -> Self {
        self.tool_output_lines = lines;
        self
    }

    pub fn thinking_lines(mut self, lines: usize) -> Self {
        self.thinking_lines = lines;
        self
    }

    pub fn show_timestamps(mut self, show: bool) -> Self {
        self.show_timestamps = show;
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
}

pub struct RenderedChatView<'a> {
    lines: &'a [Line<'static>],
    scroll_offset: usize,
}

impl<'a> RenderedChatView<'a> {
    pub fn new(lines: &'a [Line<'static>]) -> Self {
        Self {
            lines,
            scroll_offset: 0,
        }
    }

    pub fn scroll(mut self, offset: usize) -> Self {
        self.scroll_offset = offset;
        self
    }
}

impl Widget for RenderedChatView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        render_visible_lines(self.lines, area, buf, self.scroll_offset);
    }
}

impl Widget for ChatView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        if let Some(lines) = self.precomputed_lines {
            render_visible_lines(lines, area, buf, self.scroll_offset);
            return;
        }

        let (all_lines, _) = build_chat_lines(
            self.messages,
            self.theme,
            self.highlighter,
            area.width as usize,
            self.tick,
            self.tool_focus,
            self.word_wrap,
            self.chat_tool_display,
            self.tool_output_lines,
            self.thinking_lines,
            self.show_timestamps,
            self.animation_level,
            self.activity_state,
        );

        render_visible_lines(&all_lines, area, buf, self.scroll_offset);
    }
}

fn render_visible_lines(lines: &[Line<'_>], area: Rect, buf: &mut Buffer, scroll_offset: usize) {
    let window = visible_line_window(lines.len(), area.height as usize, scroll_offset);
    let visible = &lines[window.start..window.end];

    for (i, line) in visible.iter().enumerate() {
        let y = area.y.saturating_add(i as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        buf.set_line(area.x, y, line, area.width);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisibleLineWindow {
    pub(crate) scroll_offset: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

fn clamp_scroll_offset_to_view(
    total_lines: usize,
    visible_height: usize,
    scroll_offset: usize,
) -> usize {
    scroll_offset.min(total_lines.saturating_sub(visible_height))
}

pub(crate) fn visible_line_window(
    total_lines: usize,
    visible_height: usize,
    scroll_offset: usize,
) -> VisibleLineWindow {
    let scroll_offset = clamp_scroll_offset_to_view(total_lines, visible_height, scroll_offset);
    let requested = visible_height.saturating_add(scroll_offset);
    let start = total_lines.saturating_sub(requested);
    let end = total_lines.min(start.saturating_add(visible_height));

    VisibleLineWindow {
        scroll_offset,
        start,
        end,
    }
}

pub fn clamped_scroll_offset_for_total_lines(
    total_lines: usize,
    chat_area: Rect,
    scroll_offset: usize,
) -> usize {
    clamp_scroll_offset_to_view(total_lines, chat_area.height as usize, scroll_offset)
}

#[allow(clippy::too_many_arguments)]
pub fn scroll_offset_for_message_at_top(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    chat_area: Rect,
    message_index: usize,
    tick: u64,
    tool_focus: Option<usize>,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
) -> usize {
    if message_index >= messages.len() {
        return 0;
    }

    let total_lines = build_chat_render_data(
        messages,
        theme,
        highlighter,
        chat_area.width as usize,
        tick,
        tool_focus,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        animation_level,
        activity_state,
    )
    .lines
    .len();
    let anchor_line = build_chat_render_data(
        &messages[..message_index],
        theme,
        highlighter,
        chat_area.width as usize,
        tick,
        tool_focus,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        animation_level,
        activity_state,
    )
    .lines
    .len();

    let visible_height = chat_area.height as usize;
    let requested = visible_height.saturating_add(anchor_line);
    let offset = total_lines.saturating_sub(requested);
    clamp_scroll_offset_to_view(total_lines, visible_height, offset)
}

#[allow(clippy::too_many_arguments)]
pub fn clamped_scroll_offset(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    chat_area: Rect,
    scroll_offset: usize,
    tick: u64,
    tool_focus: Option<usize>,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
) -> usize {
    let render = build_chat_render_data(
        messages,
        theme,
        highlighter,
        chat_area.width as usize,
        tick,
        tool_focus,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        animation_level,
        activity_state,
    );

    clamped_scroll_offset_for_total_lines(render.lines.len(), chat_area, scroll_offset)
}

#[allow(clippy::too_many_arguments)]
pub fn build_chat_render_data(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    width: usize,
    tick: u64,
    tool_focus: Option<usize>,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
) -> ChatRenderData {
    let (lines, tool_line_indices) = build_chat_lines(
        messages,
        theme,
        highlighter,
        width,
        tick,
        tool_focus,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        animation_level,
        activity_state,
    );

    ChatRenderData {
        lines,
        tool_line_indices,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_chat_lines(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    width: usize,
    tick: u64,
    tool_focus: Option<usize>,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
) -> (Vec<Line<'static>>, Vec<(usize, String)>) {
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut tool_line_indices: Vec<(usize, String)> = Vec::new();
    let mut tool_call_counter: usize = 0;

    for (message_index, msg) in messages.iter().enumerate() {
        if show_timestamps {
            all_lines.push(Line::from(Span::styled(
                format!("  [{}]", format_timestamp(msg.timestamp)),
                theme.muted_style(),
            )));
        }

        match msg.role {
            MessageRole::User => {
                let content_style = Style::default().fg(theme.user_prefix);
                let prefix_style = Style::default()
                    .fg(theme.user_prefix)
                    .add_modifier(Modifier::BOLD);
                let logical_lines: Vec<&str> = if msg.content.is_empty() {
                    vec![""]
                } else {
                    msg.content.lines().collect()
                };

                for (idx, raw_line) in logical_lines.iter().enumerate() {
                    let prefix = if idx == 0 {
                        vec![Span::styled("❯ ".to_string(), prefix_style)]
                    } else {
                        vec![Span::styled("  ".to_string(), content_style)]
                    };
                    let continuation = vec![Span::styled("  ".to_string(), content_style)];
                    all_lines.extend(wrap_text_with_prefix(
                        raw_line,
                        &prefix,
                        &continuation,
                        content_style,
                        width,
                        word_wrap,
                    ));
                }
            }
            MessageRole::Assistant => {
                if let Some(ref thinking) = msg.thinking {
                    if !thinking.is_empty() && thinking_lines > 0 {
                        let lines: Vec<&str> = thinking.lines().collect();
                        let total = lines.len();
                        let tail = if total > thinking_lines {
                            &lines[total - thinking_lines..]
                        } else {
                            &lines[..]
                        };
                        for (i, line) in tail.iter().enumerate() {
                            let prefix = if i == 0 && total > thinking_lines {
                                "💭"
                            } else {
                                "  "
                            };
                            all_lines.extend(wrap_text_with_prefix(
                                &format!("  {prefix} {line}"),
                                &[],
                                &[],
                                theme.muted_style(),
                                width,
                                word_wrap,
                            ));
                        }
                    }
                }

                if !msg.assistant_blocks.is_empty() {
                    for block in &msg.assistant_blocks {
                        match block {
                            DisplayAssistantBlock::Text(text) => {
                                if !text.is_empty() {
                                    let rendered = markdown::render_markdown_with_width(
                                        text,
                                        theme,
                                        highlighter,
                                        width.saturating_sub(2),
                                    );
                                    let indent = vec![Span::raw("  ".to_string())];
                                    for line in rendered {
                                        all_lines.extend(wrap_line_with_prefix(
                                            &line, &indent, &indent, width, word_wrap,
                                        ));
                                    }
                                }
                            }
                            DisplayAssistantBlock::ThoughtDuration { seconds } => {
                                all_lines.extend(wrap_text_with_prefix(
                                    &format!("  thought for {}", format_duration_seconds(*seconds)),
                                    &[],
                                    &[],
                                    theme.muted_style(),
                                    width,
                                    word_wrap,
                                ));
                            }
                            DisplayAssistantBlock::ToolCall { id } => {
                                let focused = tool_focus == Some(tool_call_counter);
                                tool_call_counter += 1;
                                if let Some(tc) = msg.find_tool_call(id) {
                                    push_tool_call_chat_lines(
                                        &mut all_lines,
                                        &mut tool_line_indices,
                                        highlighter,
                                        tc,
                                        theme,
                                        tick,
                                        width,
                                        word_wrap,
                                        focused,
                                        chat_tool_display,
                                        tool_output_lines,
                                        animation_level,
                                    );
                                }
                            }
                        }
                    }
                } else {
                    if !msg.content.is_empty() {
                        let rendered = markdown::render_markdown_with_width(
                            &msg.content,
                            theme,
                            highlighter,
                            width.saturating_sub(2),
                        );
                        let indent = vec![Span::raw("  ".to_string())];
                        for line in rendered {
                            all_lines.extend(wrap_line_with_prefix(
                                &line, &indent, &indent, width, word_wrap,
                            ));
                        }
                    }
                    for tc in &msg.tool_calls {
                        let focused = tool_focus == Some(tool_call_counter);
                        tool_call_counter += 1;
                        push_tool_call_chat_lines(
                            &mut all_lines,
                            &mut tool_line_indices,
                            highlighter,
                            tc,
                            theme,
                            tick,
                            width,
                            word_wrap,
                            focused,
                            chat_tool_display,
                            tool_output_lines,
                            animation_level,
                        );
                    }
                }

                if msg.is_streaming && msg.content.trim().is_empty() {
                    let label = activity_label(
                        activity_state,
                        tick,
                        animation_level,
                        ActivitySurface::Chat,
                    );
                    if !label.is_empty() {
                        all_lines.extend(wrap_text_with_prefix(
                            &format!("  {label}"),
                            &[],
                            &[],
                            theme.accent_style(),
                            width,
                            word_wrap,
                        ));
                    }
                }
            }
            MessageRole::System => {
                for line in msg.content.lines() {
                    all_lines.extend(wrap_text_with_prefix(
                        &format!("  {line}"),
                        &[],
                        &[],
                        theme.muted_style(),
                        width,
                        word_wrap,
                    ));
                }
            }
            MessageRole::Warning => {
                push_notice_lines(
                    &mut all_lines,
                    "Warning: ",
                    &msg.content,
                    theme.warning_style().add_modifier(Modifier::BOLD),
                    theme.style(),
                    width,
                    word_wrap,
                );
            }
            MessageRole::Compaction => {
                all_lines.extend(wrap_text_with_prefix(
                    &format!("  [context compacted] {}", msg.content),
                    &[],
                    &[],
                    theme.muted_style(),
                    width,
                    word_wrap,
                ));
            }
            MessageRole::Error => {
                push_notice_lines(
                    &mut all_lines,
                    "Error: ",
                    &msg.content,
                    theme.error_style().add_modifier(Modifier::BOLD),
                    theme.style(),
                    width,
                    word_wrap,
                );
            }
        }

        let next_is_notice = messages
            .get(message_index + 1)
            .is_some_and(|next| is_notice_role(&next.role));
        if !is_notice_role(&msg.role) || !next_is_notice {
            all_lines.push(Line::raw(""));
        }
    }

    (all_lines, tool_line_indices)
}

fn is_notice_role(role: &MessageRole) -> bool {
    matches!(role, MessageRole::Warning | MessageRole::Error)
}

fn push_notice_lines(
    all_lines: &mut Vec<Line<'static>>,
    label: &str,
    content: &str,
    label_style: Style,
    body_style: Style,
    width: usize,
    word_wrap: bool,
) {
    let logical_lines = content.lines().collect::<Vec<_>>();
    let logical_lines = if logical_lines.is_empty() {
        vec![""]
    } else {
        logical_lines
    };
    let continuation = vec![Span::raw("  ".to_string())];
    for (index, text) in logical_lines.into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 {
            spans.push(Span::styled(label.to_string(), label_style));
        } else {
            spans.push(Span::raw("  ".to_string()));
        }
        spans.push(Span::styled(text.to_string(), body_style));
        let line = Line::from(spans);
        all_lines.extend(wrap_line_with_prefix(
            &line,
            &[],
            &continuation,
            width,
            word_wrap,
        ));
    }
}

fn format_duration_seconds(seconds: u64) -> String {
    match seconds {
        0 | 1 => "1 second".to_string(),
        2..=59 => format!("{seconds} seconds"),
        _ => {
            let minutes = seconds / 60;
            let remaining_seconds = seconds % 60;
            if remaining_seconds == 0 {
                format!("{minutes}m")
            } else {
                format!("{minutes}m {remaining_seconds}s")
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_tool_call_chat_lines(
    all_lines: &mut Vec<Line<'static>>,
    tool_line_indices: &mut Vec<(usize, String)>,
    highlighter: &Highlighter,
    tc: &DisplayToolCall,
    theme: &Theme,
    tick: u64,
    width: usize,
    word_wrap: bool,
    focused: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    animation_level: AnimationLevel,
) {
    if chat_tool_display == ChatToolDisplay::Hidden {
        return;
    }

    let is_running = tc.output.is_none() && !tc.is_error;
    let rail = vec![Span::styled("  ".to_string(), theme.muted_style())];
    let header = tc.header_line_animated_focused(theme, tick, focused, animation_level);
    let header_lines = wrap_line_with_prefix(&header, &rail, &rail, width, word_wrap);
    let header_start = all_lines.len();
    for offset in 0..header_lines.len() {
        tool_line_indices.push((header_start + offset, tc.id.clone()));
    }
    all_lines.extend(header_lines);
    let detail_lines = tc.summary_detail_lines(theme);
    if chat_tool_display == ChatToolDisplay::Summary && !detail_lines.is_empty() {
        for line in detail_lines {
            let line_start = all_lines.len();
            let wrapped = wrap_line_with_prefix(&line, &rail, &rail, width, word_wrap);
            for offset in 0..wrapped.len() {
                tool_line_indices.push((line_start + offset, tc.id.clone()));
            }
            all_lines.extend(wrapped);
        }
    }

    if chat_tool_display == ChatToolDisplay::Summary {
        return;
    }

    if is_running && !tc.streaming_lines.is_empty() {
        let mut rendered = Vec::new();
        for line in &tc.streaming_lines {
            let content = Line::from(Span::styled(format!("    {line}"), theme.muted_style()));
            rendered.extend(wrap_line_with_prefix(
                &content, &rail, &rail, width, word_wrap,
            ));
        }
        let rendered = if matches!(tc.name.as_str(), "bash" | "shell") {
            let budget = tool_output_lines.clamp(1, 5);
            let start = rendered.len().saturating_sub(budget);
            rendered.into_iter().skip(start).collect()
        } else {
            rendered
        };
        append_tool_rows(all_lines, tool_line_indices, &tc.id, rendered.into_iter());
    }

    if tc.expanded {
        let output_lines = styled_tool_output_lines(tc, highlighter, theme, tc.name == "read");
        let mut rendered = Vec::new();
        for line in output_lines {
            rendered.extend(wrap_line_with_prefix(&line, &rail, &rail, width, word_wrap));
        }
        let limited = if matches!(tc.name.as_str(), "bash" | "shell") {
            limit_shell_rows(rendered, tool_output_lines, tc.is_error, theme)
        } else {
            rendered.into_iter().take(50).collect()
        };
        append_tool_rows(all_lines, tool_line_indices, &tc.id, limited.into_iter());
    }
}

fn append_tool_rows(
    all_lines: &mut Vec<Line<'static>>,
    tool_line_indices: &mut Vec<(usize, String)>,
    tool_call_id: &str,
    rows: impl Iterator<Item = Line<'static>>,
) {
    for line in rows {
        tool_line_indices.push((all_lines.len(), tool_call_id.to_string()));
        all_lines.push(line);
    }
}

fn limit_shell_rows(
    rows: Vec<Line<'static>>,
    budget: usize,
    is_error: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let budget = budget.max(1);
    if rows.len() <= budget {
        return rows;
    }

    let marker_rows = 1;
    let content_budget = budget.saturating_sub(marker_rows);
    let head_rows = if is_error {
        0
    } else {
        content_budget.saturating_sub(1).min(4)
    };
    let tail_rows = content_budget.saturating_sub(head_rows);
    let omitted = rows.len().saturating_sub(head_rows + tail_rows);
    let mut limited = Vec::with_capacity(budget);
    limited.extend(rows.iter().take(head_rows).cloned());
    limited.push(Line::from(Span::styled(
        format!("    … {omitted} rows omitted · open inspector for full output"),
        theme.muted_style(),
    )));
    limited.extend(rows.into_iter().skip(head_rows + omitted));
    limited
}

fn wrap_text_with_prefix(
    text: &str,
    first_prefix: &[Span<'_>],
    continuation_prefix: &[Span<'_>],
    style: Style,
    width: usize,
    enabled: bool,
) -> Vec<Line<'static>> {
    let content = Line::from(Span::styled(text.to_string(), style));
    wrap_line_with_prefix(&content, first_prefix, continuation_prefix, width, enabled)
}

fn wrap_line_with_prefix(
    line: &Line<'_>,
    first_prefix: &[Span<'_>],
    continuation_prefix: &[Span<'_>],
    width: usize,
    enabled: bool,
) -> Vec<Line<'static>> {
    let first_prefix_owned = clone_spans(first_prefix);
    let continuation_prefix_owned = clone_spans(continuation_prefix);

    if !enabled || width == 0 {
        let mut spans = first_prefix_owned;
        spans.extend(clone_spans(&line.spans));
        return vec![Line::from(spans)];
    }

    let chars = flatten_line_chars(line);
    if chars.is_empty() {
        return vec![Line::from(first_prefix_owned)];
    }

    let first_width = width.saturating_sub(spans_width(first_prefix));
    let continuation_width = width.saturating_sub(spans_width(continuation_prefix));
    let chunks = wrap_styled_chars(&chars, first_width, continuation_width);

    let mut lines = Vec::with_capacity(chunks.len());
    for (idx, chunk) in chunks.into_iter().enumerate() {
        let mut spans = if idx == 0 {
            clone_spans(&first_prefix_owned)
        } else {
            clone_spans(&continuation_prefix_owned)
        };
        spans.extend(chars_to_spans(&chunk));
        lines.push(Line::from(spans));
    }

    lines
}

fn clone_spans(spans: &[Span<'_>]) -> Vec<Span<'static>> {
    spans
        .iter()
        .map(|span| Span::styled(span.content.to_string(), span.style))
        .collect()
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>()
}

fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn flatten_line_chars(line: &Line<'_>) -> Vec<(char, Style)> {
    let mut chars = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            chars.push((ch, span.style));
        }
    }
    chars
}

fn wrap_styled_chars(
    chars: &[(char, Style)],
    first_width: usize,
    continuation_width: usize,
) -> Vec<Vec<(char, Style)>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut current_width = first_width.max(1);

    while start < chars.len() {
        let remaining = chars.len() - start;
        if remaining <= current_width {
            chunks.push(chars[start..].to_vec());
            break;
        }

        let end = start + current_width;
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

        current_width = continuation_width.max(1);
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

/// Calculate the total number of rendered lines across all messages.
pub fn total_rendered_lines(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
) -> usize {
    messages
        .iter()
        .map(|m| m.line_count(theme, highlighter))
        .sum()
}

fn format_timestamp(ts: u64) -> String {
    let secs = ts % 86_400;
    let h = secs / 3_600;
    let m = (secs % 3_600) / 60;
    format!("{h:02}:{m:02}")
}

pub fn build_text_surface_from_lines(
    lines: &[Line<'_>],
    chat_area: Rect,
    scroll_offset: usize,
) -> TextSurface {
    let lines: Vec<String> = lines.iter().map(line_to_plain_text).collect();
    let total_lines = lines.len();
    let start = visible_line_window(total_lines, chat_area.height as usize, scroll_offset).start;

    TextSurface::new(
        crate::selection::SelectablePane::Chat,
        chat_area,
        lines,
        start,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_text_surface(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    chat_area: Rect,
    scroll_offset: usize,
    tick: u64,
    tool_focus: Option<usize>,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
    animation_level: AnimationLevel,
    activity_state: AnimationState,
) -> TextSurface {
    let render = build_chat_render_data(
        messages,
        theme,
        highlighter,
        chat_area.width as usize,
        tick,
        tool_focus,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        animation_level,
        activity_state,
    );

    build_text_surface_from_lines(&render.lines, chat_area, scroll_offset)
}

/// Build a click map from already-rendered chat lines.
pub fn build_click_map_from_rendered_lines(
    lines: &[Line<'_>],
    chat_area: Rect,
    scroll_offset: usize,
) -> Vec<(u16, String)> {
    let total_lines = lines.len();
    let window = visible_line_window(total_lines, chat_area.height as usize, scroll_offset);
    let mut result = Vec::new();

    for (line_index, line) in lines.iter().enumerate().take(window.end).skip(window.start) {
        let plain = line_to_plain_text(line);
        let header = plain.trim_start();
        let Some(rest) = header
            .strip_prefix("▸ ")
            .or_else(|| header.strip_prefix("▾ "))
        else {
            continue;
        };
        let Some(id) = rest.strip_prefix('#') else {
            continue;
        };
        let id = id.split_whitespace().next().unwrap_or_default();
        if !id.is_empty() {
            let screen_y = chat_area
                .y
                .saturating_add((line_index - window.start) as u16);
            result.push((screen_y, id.to_string()));
        }
    }

    result
}

#[allow(clippy::too_many_arguments)]
pub fn build_click_map(
    messages: &[DisplayMessage],
    theme: &Theme,
    highlighter: &Highlighter,
    chat_area: Rect,
    scroll_offset: usize,
    word_wrap: bool,
    chat_tool_display: ChatToolDisplay,
    tool_output_lines: usize,
    thinking_lines: usize,
    show_timestamps: bool,
) -> Vec<(u16, String)> {
    let (all_lines, tool_line_indices) = build_chat_lines(
        messages,
        theme,
        highlighter,
        chat_area.width as usize,
        0,
        None,
        word_wrap,
        chat_tool_display,
        tool_output_lines,
        thinking_lines,
        show_timestamps,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let window = visible_line_window(all_lines.len(), chat_area.height as usize, scroll_offset);

    let mut result = Vec::new();
    for (line_index, id) in &tool_line_indices {
        if *line_index >= window.start && *line_index < window.end {
            let screen_y = chat_area
                .y
                .saturating_add((*line_index - window.start) as u16);
            result.push((screen_y, id.clone()));
        }
    }

    result
}

#[cfg(test)]
#[path = "chat/tests.rs"]
mod tests;
