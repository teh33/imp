use std::collections::HashMap;

use imp_llm::{truncate_chars_with_suffix, ContentBlock, Message, Model, ModelMeta};

fn truncate_for_display(text: &str, max_chars: usize) -> String {
    truncate_chars_with_suffix(text, max_chars, "...")
}

/// Context usage stats.
#[derive(Debug, Clone)]
pub struct ContextUsage {
    pub used: u32,
    pub limit: u32,
    pub ratio: f64,
}

/// Resolved budget used for context display and preflight checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudget {
    /// Provider/model maximum total context, including reserved response or
    /// recovery room when known.
    pub total_window: u32,
    /// User-visible denominator and normal input budget.
    pub display_window: u32,
    /// Tokens intentionally held back for output, summarization, or recovery.
    pub reserved_buffer: u32,
}

impl ContextBudget {
    pub fn ratio_for_used(&self, used: u32) -> f64 {
        if self.display_window > 0 {
            used as f64 / self.display_window as f64
        } else {
            0.0
        }
    }
}

/// Resolve the budget for a model metadata entry.
pub fn context_budget_for_meta(meta: &imp_llm::ModelMeta) -> ContextBudget {
    if meta.id == "gpt-5.5" {
        return ContextBudget {
            total_window: 1_050_000,
            display_window: 1_000_000,
            reserved_buffer: 50_000,
        };
    }

    ContextBudget {
        total_window: meta.context_window,
        display_window: meta.context_window,
        reserved_buffer: 0,
    }
}

/// Resolve the budget for a runtime model.
pub fn context_budget(model: &Model) -> ContextBudget {
    context_budget_for_meta(&model.meta)
}

/// Fast approximate token counting (~4 chars per token for English).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() as u32) / 4
}

fn count_openai_tokens(text: &str, model_id: &str) -> Option<u32> {
    let bpe = tiktoken_rs::bpe_for_model(model_id)
        .or_else(|_| tiktoken_rs::bpe_for_model("gpt-5"))
        .ok()?;
    Some(bpe.encode_with_special_tokens(text).len() as u32)
}

fn model_uses_openai_tokenizer(meta: &ModelMeta) -> bool {
    matches!(meta.provider.as_str(), "openai" | "openai-codex")
        || meta.id.starts_with("gpt-")
        || meta.id.starts_with('o')
}

/// Estimate text tokens, using OpenAI-family tokenizers when available.
pub fn estimate_text_tokens_for_model(text: &str, meta: &ModelMeta) -> u32 {
    if model_uses_openai_tokenizer(meta) {
        return count_openai_tokens(text, &meta.id).unwrap_or_else(|| estimate_tokens(text));
    }

    estimate_tokens(text)
}

/// Estimate one message's contribution to provider request input.
pub fn estimate_message_tokens_for_model(message: &Message, meta: &ModelMeta) -> u32 {
    if model_uses_openai_tokenizer(meta) {
        match message {
            Message::User(user) => {
                4 + user
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => estimate_text_tokens_for_model(text, meta),
                        ContentBlock::Thinking { text } => {
                            estimate_text_tokens_for_model(text, meta)
                        }
                        ContentBlock::ToolCall {
                            name, arguments, ..
                        } => {
                            8 + estimate_text_tokens_for_model(name, meta)
                                + estimate_text_tokens_for_model(
                                    &serde_json::to_string(arguments).unwrap_or_default(),
                                    meta,
                                )
                        }
                        ContentBlock::Image { data, .. } => estimate_tokens(data),
                    })
                    .sum::<u32>()
            }
            Message::Assistant(assistant) => {
                4 + assistant
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => estimate_text_tokens_for_model(text, meta),
                        ContentBlock::Thinking { text } => {
                            estimate_text_tokens_for_model(text, meta)
                        }
                        ContentBlock::ToolCall {
                            name, arguments, ..
                        } => {
                            8 + estimate_text_tokens_for_model(name, meta)
                                + estimate_text_tokens_for_model(
                                    &serde_json::to_string(arguments).unwrap_or_default(),
                                    meta,
                                )
                        }
                        ContentBlock::Image { data, .. } => estimate_tokens(data),
                    })
                    .sum::<u32>()
            }
            Message::ToolResult(result) => {
                6 + estimate_text_tokens_for_model(&result.tool_name, meta)
                    + result
                        .content
                        .iter()
                        .map(|block| match block {
                            ContentBlock::Text { text } => {
                                estimate_text_tokens_for_model(text, meta)
                            }
                            ContentBlock::Thinking { text } => {
                                estimate_text_tokens_for_model(text, meta)
                            }
                            ContentBlock::ToolCall {
                                name, arguments, ..
                            } => {
                                8 + estimate_text_tokens_for_model(name, meta)
                                    + estimate_text_tokens_for_model(
                                        &serde_json::to_string(arguments).unwrap_or_default(),
                                        meta,
                                    )
                            }
                            ContentBlock::Image { data, .. } => estimate_tokens(data),
                        })
                        .sum::<u32>()
            }
        }
    } else {
        let json = serde_json::to_string(message).unwrap_or_default();
        estimate_tokens(&json)
    }
}

/// Estimate total context usage for a message list.
pub fn context_usage(messages: &[Message], model: &Model) -> ContextUsage {
    let used: u32 = messages
        .iter()
        .map(|m| estimate_message_tokens_for_model(m, &model.meta))
        .sum();
    let budget = context_budget(model);
    let limit = budget.display_window;
    let ratio = budget.ratio_for_used(used);
    ContextUsage { used, limit, ratio }
}

/// Replace old tool result content with lightweight placeholders.
///
/// A "turn" is one assistant message plus its following tool results.
/// Keeps the last `keep_recent_turns` turns fully intact. For older turns,
/// tool result content is replaced with a summary placeholder preserving
/// the tool name, a truncated summary of args, and the byte count. Passing
/// `keep_recent_turns = 0` masks all observed tool results.
pub fn mask_observations(messages: &mut [Message], keep_recent_turns: usize) {
    // Identify turn boundaries — each assistant message starts a new turn.
    let turn_starts: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_assistant())
        .map(|(i, _)| i)
        .collect();

    if turn_starts.len() <= keep_recent_turns {
        return;
    }

    // Everything before this message index gets masked.
    let cutoff_msg_idx = if keep_recent_turns == 0 {
        messages.len()
    } else {
        let cutoff_turn = turn_starts.len() - keep_recent_turns;
        turn_starts[cutoff_turn]
    };

    // Build a map of tool_call_id → args summary from assistant ToolCall blocks
    // in the region we're about to mask.
    let mut args_map: HashMap<String, String> = HashMap::new();
    for msg in &messages[..cutoff_msg_idx] {
        if let Message::Assistant(assistant) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall { id, arguments, .. } = block {
                    let args_json = serde_json::to_string(arguments).unwrap_or_default();
                    let summary = truncate_for_display(&args_json, 100);
                    args_map.insert(id.clone(), summary);
                }
            }
        }
    }

    // Replace tool result content with placeholders.
    for msg in &mut messages[..cutoff_msg_idx] {
        if let Message::ToolResult(ref mut result) = msg {
            let byte_count: usize = result
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    _ => 0,
                })
                .sum();

            let args_summary = args_map
                .get(&result.tool_call_id)
                .map(|s| s.as_str())
                .unwrap_or("");

            let placeholder = format!(
                "[Output omitted — ran {}({}), returned {} bytes]",
                result.tool_name, args_summary, byte_count
            );
            result.content = vec![ContentBlock::Text { text: placeholder }];
        }
    }
}

#[cfg(test)]
mod tests;
