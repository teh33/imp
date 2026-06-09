use std::collections::HashMap;

use imp_llm::{truncate_chars_with_suffix, ContentBlock, Message};

const MAX_TOOL_ARGS_DIGEST_CHARS: usize = 100;
const MAX_TOOL_OUTPUT_DIGEST_CHARS: usize = 600;

fn truncate_for_digest(text: &str, max_chars: usize) -> String {
    truncate_chars_with_suffix(text, max_chars, "...")
}

fn assistant_turn_starts(messages: &[Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.is_assistant())
        .map(|(index, _)| index)
        .collect()
}

fn cutoff_for_recent_assistant_turns(
    messages: &[Message],
    keep_recent_assistant_turns: usize,
) -> Option<usize> {
    let turn_starts = assistant_turn_starts(messages);
    if keep_recent_assistant_turns == 0 {
        return (!turn_starts.is_empty()).then_some(messages.len());
    }
    if turn_starts.len() <= keep_recent_assistant_turns {
        return None;
    }
    let cutoff_turn = turn_starts
        .len()
        .saturating_sub(keep_recent_assistant_turns);
    turn_starts.get(cutoff_turn).copied()
}

fn tool_call_args_before(messages: &[Message], end_index: usize) -> HashMap<String, String> {
    let mut args_by_call_id = HashMap::new();
    for message in &messages[..end_index] {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        for block in &assistant.content {
            if let ContentBlock::ToolCall { id, arguments, .. } = block {
                let args_json = serde_json::to_string(arguments).unwrap_or_default();
                args_by_call_id.insert(
                    id.clone(),
                    truncate_for_digest(&args_json, MAX_TOOL_ARGS_DIGEST_CHARS),
                );
            }
        }
    }
    args_by_call_id
}

fn latest_tool_call_ids(messages: &[Message]) -> Vec<String> {
    for message in messages.iter().rev() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        return assistant
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
    }
    Vec::new()
}

fn tool_result_text_bytes(result: &imp_llm::ToolResultMessage) -> usize {
    result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            _ => 0,
        })
        .sum()
}

fn tool_result_text_excerpt(result: &imp_llm::ToolResultMessage) -> Option<String> {
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| truncate_for_digest(text, MAX_TOOL_OUTPUT_DIGEST_CHARS))
}

fn digest_text(
    result: &imp_llm::ToolResultMessage,
    args_by_call_id: &HashMap<String, String>,
) -> String {
    let byte_count = tool_result_text_bytes(result);
    let args_summary = args_by_call_id
        .get(&result.tool_call_id)
        .map(String::as_str)
        .unwrap_or("");
    let status = if result.is_error {
        "failed"
    } else {
        "succeeded"
    };
    let mut digest = format!(
        "[Output omitted — ran {}({}), returned {} bytes]",
        result.tool_name, args_summary, byte_count
    );
    if let Some(excerpt) = tool_result_text_excerpt(result) {
        digest.push_str("\nstatus: ");
        digest.push_str(status);
        digest.push_str("\nexcerpt:\n");
        digest.push_str(&excerpt);
    }
    digest
}

/// Return a cache-conscious provider-context projection with old tool-result
/// bodies replaced by stable digests.
///
/// This is a pure version of the 0.1.2 observation masking behavior. It keeps
/// the recent assistant-turn tail verbatim, preserves the latest assistant
/// tool-call/result protocol pair even when the requested tail is tiny, and
/// leaves raw session history ownership to the caller.
pub fn digest_old_tool_results(
    messages: &[Message],
    keep_recent_assistant_turns: usize,
) -> Vec<Message> {
    let Some(cutoff_index) =
        cutoff_for_recent_assistant_turns(messages, keep_recent_assistant_turns)
    else {
        return messages.to_vec();
    };

    let mut projected = messages.to_vec();
    let args_by_call_id = tool_call_args_before(messages, cutoff_index);
    let latest_call_ids = latest_tool_call_ids(messages);

    for message in &mut projected[..cutoff_index] {
        let Message::ToolResult(result) = message else {
            continue;
        };
        if latest_call_ids.contains(&result.tool_call_id) {
            continue;
        }
        let digest = digest_text(result, &args_by_call_id);
        result.content = vec![ContentBlock::Text { text: digest }];
    }

    projected
}

#[cfg(test)]
mod tests {
    use imp_llm::{AssistantMessage, StopReason, ToolResultMessage};

    use super::*;

    fn assistant_tool_call(call_id: &str, tool_name: &str, args: serde_json::Value) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments: args,
            }],
            usage: None,
            stop_reason: StopReason::ToolUse,
            timestamp: 1000,
        })
    }

    fn assistant_text(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 1000,
        })
    }

    fn tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![ContentBlock::Text {
                text: output.to_string(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: 1000,
        })
    }

    fn error_tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![ContentBlock::Text {
                text: output.to_string(),
            }],
            is_error: true,
            details: serde_json::Value::Null,
            timestamp: 1000,
        })
    }

    fn tool_result_text(message: &Message) -> &str {
        let Message::ToolResult(result) = message else {
            panic!("expected tool result");
        };
        let ContentBlock::Text { text } = &result.content[0] else {
            panic!("expected text block");
        };
        text
    }

    #[test]
    fn digests_old_tool_results_and_preserves_recent_tail() {
        let messages = vec![
            Message::user("start"),
            assistant_tool_call("old", "read", serde_json::json!({"path":"old.rs"})),
            tool_result("old", "read", "old output"),
            assistant_tool_call("new", "read", serde_json::json!({"path":"new.rs"})),
            tool_result("new", "read", "new output"),
        ];

        let projected = digest_old_tool_results(&messages, 1);

        assert!(tool_result_text(&projected[2]).starts_with("[Output omitted"));
        assert_eq!(tool_result_text(&projected[4]), "new output");
    }

    #[test]
    fn preserves_latest_tool_pair_even_with_no_recent_tail() {
        let messages = vec![
            Message::user("start"),
            assistant_tool_call("old", "read", serde_json::json!({"path":"old.rs"})),
            tool_result("old", "read", "old output"),
            assistant_tool_call(
                "latest",
                "bash",
                serde_json::json!({"command":"cargo test"}),
            ),
            tool_result("latest", "bash", "latest output"),
        ];

        let projected = digest_old_tool_results(&messages, 0);

        assert!(tool_result_text(&projected[2]).starts_with("[Output omitted"));
        assert_eq!(tool_result_text(&projected[4]), "latest output");
    }

    #[test]
    fn projection_is_deterministic() {
        let messages = vec![
            Message::user("start"),
            assistant_tool_call("old", "read", serde_json::json!({"path":"old.rs"})),
            tool_result("old", "read", "old output"),
            assistant_text("done"),
        ];

        let first = serde_json::to_value(digest_old_tool_results(&messages, 1)).unwrap();
        let second = serde_json::to_value(digest_old_tool_results(&messages, 1)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn digest_keeps_bounded_failure_evidence() {
        let messages = vec![
            Message::user("start"),
            assistant_tool_call("old", "bash", serde_json::json!({"command":"cargo test"})),
            error_tool_result("old", "bash", "test failed: expected ready step"),
            assistant_text("done"),
        ];

        let projected = digest_old_tool_results(&messages, 1);

        let digest = tool_result_text(&projected[2]);
        assert!(digest.contains("status: failed"));
        assert!(digest.contains("test failed: expected ready step"));
    }
}
