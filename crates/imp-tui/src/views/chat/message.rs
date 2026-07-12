use crate::highlight::Highlighter;
use crate::markdown;
use crate::theme::Theme;
use crate::views::tools::{tool_call_height, DisplayToolCall};

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

    pub(super) fn find_tool_call(&self, id: &str) -> Option<&DisplayToolCall> {
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
