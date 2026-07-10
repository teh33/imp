use imp_llm::{ContentBlock, Message};

use super::{
    RuntimeAssistantBlock, RuntimeMessageRole, RuntimeToolCall, RuntimeToolStatus,
    RuntimeTranscriptMessage, MAX_RUNTIME_TOOL_OUTPUT_CHARS,
};
use crate::compaction::COMPACTION_SUMMARY_PREFIX;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeSessionProjection {
    pub transcript: Vec<RuntimeTranscriptMessage>,
    pub completed_tools: Vec<RuntimeToolCall>,
}

impl RuntimeSessionProjection {
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut projection = Self::default();
        for (index, message) in messages.iter().enumerate() {
            projection.push_message(index, message);
        }
        projection
    }

    fn push_message(&mut self, index: usize, message: &Message) {
        match message {
            Message::User(message) => {
                let role = if visible_text(&message.content).starts_with(COMPACTION_SUMMARY_PREFIX)
                {
                    RuntimeMessageRole::Compaction
                } else {
                    RuntimeMessageRole::User
                };
                self.transcript.push(runtime_message(
                    format!("session-user-{index}"),
                    role,
                    &message.content,
                    message.timestamp,
                ));
            }
            Message::Assistant(message) => {
                let blocks = message
                    .content
                    .iter()
                    .filter_map(|block| self.assistant_block(block))
                    .collect();
                self.transcript.push(RuntimeTranscriptMessage {
                    id: format!("session-assistant-{index}"),
                    role: RuntimeMessageRole::Assistant,
                    blocks,
                    timestamp_ms: Some(message.timestamp.saturating_mul(1000)),
                    ..RuntimeTranscriptMessage::default()
                });
            }
            Message::ToolResult(result) => self.attach_tool_result(index, result),
        }
    }

    fn assistant_block(&mut self, block: &ContentBlock) -> Option<RuntimeAssistantBlock> {
        match block {
            ContentBlock::Text { text } => {
                Some(RuntimeAssistantBlock::VisibleText { text: text.clone() })
            }
            ContentBlock::Thinking { text } => {
                Some(RuntimeAssistantBlock::Thinking { text: text.clone() })
            }
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => {
                let arguments = crate::agent::redact_runtime_value(arguments);
                self.completed_tools.push(RuntimeToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    status: RuntimeToolStatus::Pending,
                    args_preview: Some(arguments.to_string()),
                    arguments: Some(arguments),
                    ..RuntimeToolCall::default()
                });
                Some(RuntimeAssistantBlock::ToolCall {
                    tool_call_id: id.clone(),
                })
            }
            ContentBlock::Image { .. } => None,
        }
    }

    fn attach_tool_result(&mut self, index: usize, result: &imp_llm::ToolResultMessage) {
        let output = visible_text(&result.content);
        if let Some(tool) = self
            .completed_tools
            .iter_mut()
            .rev()
            .find(|tool| tool.id == result.tool_call_id)
        {
            tool.name = result.tool_name.clone();
            tool.status = if result.is_error {
                RuntimeToolStatus::Failed
            } else {
                RuntimeToolStatus::Succeeded
            };
            tool.output_preview = Some(bounded_tail(&output, MAX_RUNTIME_TOOL_OUTPUT_CHARS));
            tool.details = Some(crate::agent::redact_runtime_value(&result.details));
            tool.is_error = result.is_error;
            tool.completed_at_ms = Some(result.timestamp.saturating_mul(1000));
            return;
        }
        self.transcript.push(runtime_message(
            format!("session-tool-result-{index}"),
            RuntimeMessageRole::ToolResult,
            &result.content,
            result.timestamp,
        ));
    }
}

fn runtime_message(
    id: String,
    role: RuntimeMessageRole,
    content: &[ContentBlock],
    timestamp: u64,
) -> RuntimeTranscriptMessage {
    RuntimeTranscriptMessage {
        id,
        role,
        blocks: runtime_blocks(content),
        timestamp_ms: Some(timestamp.saturating_mul(1000)),
        ..RuntimeTranscriptMessage::default()
    }
}

fn runtime_blocks(content: &[ContentBlock]) -> Vec<RuntimeAssistantBlock> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => {
                Some(RuntimeAssistantBlock::VisibleText { text: text.clone() })
            }
            ContentBlock::Thinking { text } => {
                Some(RuntimeAssistantBlock::Thinking { text: text.clone() })
            }
            ContentBlock::ToolCall { id, .. } => Some(RuntimeAssistantBlock::ToolCall {
                tool_call_id: id.clone(),
            }),
            ContentBlock::Image { .. } => None,
        })
        .collect()
}

fn visible_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn bounded_tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail = text.chars().skip(count - max_chars).collect::<String>();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use imp_llm::{AssistantMessage, StopReason, ToolResultMessage, UserMessage};

    use super::*;

    #[test]
    fn projects_ordered_session_blocks_and_tool_results() {
        let messages = vec![
            Message::User(UserMessage {
                content: vec![ContentBlock::Text { text: "hi".into() }],
                timestamp: 1,
            }),
            Message::Assistant(AssistantMessage {
                content: vec![
                    ContentBlock::Text { text: "a".into() },
                    ContentBlock::ToolCall {
                        id: "tool".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"token": "secret", "command": "pwd"}),
                    },
                    ContentBlock::Text { text: "b".into() },
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
                timestamp: 2,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "tool".into(),
                tool_name: "bash".into(),
                content: vec![ContentBlock::Text {
                    text: "/tmp".into(),
                }],
                is_error: false,
                details: serde_json::json!({"authorization": "secret"}),
                timestamp: 3,
            }),
        ];

        let projection = RuntimeSessionProjection::from_messages(&messages);
        assert_eq!(projection.transcript.len(), 2);
        assert_eq!(projection.transcript[1].visible_text(), "ab");
        assert!(matches!(
            projection.transcript[1].blocks[1],
            RuntimeAssistantBlock::ToolCall { .. }
        ));
        assert_eq!(
            projection.completed_tools[0].output_preview.as_deref(),
            Some("/tmp")
        );
        assert_eq!(
            projection.completed_tools[0].arguments.as_ref().unwrap()["token"],
            "[redacted]"
        );
        assert_eq!(
            projection.completed_tools[0].details.as_ref().unwrap()["authorization"],
            "[redacted]"
        );
    }
}
