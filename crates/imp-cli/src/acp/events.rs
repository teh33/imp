use imp_llm::message::{ContentBlock as LlmContentBlock, Message};

use super::protocol::{
    ContentBlock, EmbeddedResource, SessionUpdate, StopReason, ToolCallContent, ToolCallStatus,
};

pub(crate) fn prompt_blocks_to_text(blocks: &[ContentBlock]) -> Result<String, String> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => parts.push(text.clone()),
            ContentBlock::Resource { resource } => {
                let text = resource.text.as_deref().ok_or_else(|| {
                    format!(
                        "unsupported resource without embedded text: {}",
                        resource.uri
                    )
                })?;
                parts.push(format!(
                    "<resource uri=\"{}\"{}>\n{}\n</resource>",
                    resource.uri,
                    resource
                        .mime_type
                        .as_ref()
                        .map(|mime| format!(" mime_type=\"{mime}\""))
                        .unwrap_or_default(),
                    text
                ));
            }
            ContentBlock::ResourceLink { uri, name } => {
                let label = name.as_deref().unwrap_or(uri);
                parts.push(format!("Resource link: {label} ({uri})"));
            }
            ContentBlock::Unknown => {
                return Err("unsupported ACP content block type".to_string());
            }
        }
    }

    Ok(parts.join("\n\n"))
}

pub(crate) fn message_to_session_updates(
    message: &Message,
    from_history: bool,
) -> Vec<SessionUpdate> {
    match message {
        Message::User(user) => user
            .content
            .iter()
            .filter_map(llm_content_to_acp)
            .map(|content| SessionUpdate::UserMessageChunk { content })
            .collect(),
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .filter_map(llm_content_to_acp)
            .map(|content| SessionUpdate::AgentMessageChunk { content })
            .collect(),
        Message::ToolResult(result) if from_history => vec![SessionUpdate::ToolCallUpdate {
            tool_call_id: result.tool_call_id.clone(),
            status: Some(if result.is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            }),
            content: result
                .content
                .iter()
                .filter_map(llm_content_to_acp)
                .map(|content| ToolCallContent::Content { content })
                .collect(),
            raw_output: Some(result.details.clone()),
        }],
        Message::ToolResult(_) => Vec::new(),
    }
}

pub(crate) fn stop_reason_from_agent_status(
    status: Option<&imp_core::agent::RunFinalStatus>,
    cancelled: bool,
) -> StopReason {
    if cancelled {
        return StopReason::Cancelled;
    }

    match status {
        Some(imp_core::agent::RunFinalStatus::Failed { .. }) => StopReason::Refusal,
        Some(imp_core::agent::RunFinalStatus::Blocked { .. }) => StopReason::Refusal,
        _ => StopReason::EndTurn,
    }
}

fn llm_content_to_acp(block: &LlmContentBlock) -> Option<ContentBlock> {
    match block {
        LlmContentBlock::Text { text } | LlmContentBlock::Thinking { text } => {
            Some(ContentBlock::Text { text: text.clone() })
        }
        LlmContentBlock::ToolCall {
            id,
            name,
            arguments,
        } => Some(ContentBlock::Resource {
            resource: EmbeddedResource {
                uri: format!("imp://tool-call/{id}"),
                mime_type: Some("application/json".to_string()),
                text: Some(
                    serde_json::json!({
                        "id": id,
                        "name": name,
                        "arguments": arguments,
                    })
                    .to_string(),
                ),
            },
        }),
        LlmContentBlock::Image { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imp_llm::message::{AssistantMessage, StopReason as LlmStopReason, UserMessage};

    #[test]
    fn prompt_blocks_to_text_includes_embedded_resource() {
        let prompt = prompt_blocks_to_text(&[
            ContentBlock::Text {
                text: "Review this".to_string(),
            },
            ContentBlock::Resource {
                resource: EmbeddedResource {
                    uri: "file:///tmp/main.rs".to_string(),
                    mime_type: Some("text/rust".to_string()),
                    text: Some("fn main() {}".to_string()),
                },
            },
        ])
        .unwrap();

        assert!(prompt.contains("Review this"));
        assert!(prompt.contains("file:///tmp/main.rs"));
        assert!(prompt.contains("fn main() {}"));
    }

    #[test]
    fn message_to_session_updates_maps_user_and_assistant_text() {
        let user = Message::User(UserMessage {
            content: vec![LlmContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 1,
        });
        let assistant = Message::Assistant(AssistantMessage {
            content: vec![LlmContentBlock::Text {
                text: "hello".to_string(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 2,
        });

        assert!(matches!(
            &message_to_session_updates(&user, false)[0],
            SessionUpdate::UserMessageChunk { content: ContentBlock::Text { text } } if text == "hi"
        ));
        assert!(matches!(
            &message_to_session_updates(&assistant, false)[0],
            SessionUpdate::AgentMessageChunk { content: ContentBlock::Text { text } } if text == "hello"
        ));
    }
}
