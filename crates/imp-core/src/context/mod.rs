pub mod projection;

use imp_llm::{ContentBlock, Message, Model, ModelMeta, RequestOptions, ToolDefinition};

/// Context usage stats.
#[derive(Debug, Clone)]
pub struct ContextUsage {
    pub used: u32,
    pub limit: u32,
    pub ratio: f64,
}

/// Request-size estimate for the exact provider-bound input candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContextEstimate {
    /// Estimated input tokens sent to the provider: system prompt, tool
    /// definitions, active messages, tool calls/results, and multimodal payloads.
    pub input_tokens: u32,
    /// Estimated system-prompt input tokens.
    pub system_tokens: u32,
    /// Estimated tool-definition/schema input tokens.
    pub tool_definition_tokens: u32,
    /// Estimated active-message input tokens.
    pub message_tokens: u32,
    /// Input-token budget used for preflight checks.
    pub input_limit: u32,
    /// Planned or provider-default output cap for diagnostics. This is not
    /// included in `input_tokens`; models with separate input/output limits use
    /// `input_limit` for prompt preflight.
    pub output_tokens: u32,
    /// User-visible display denominator.
    pub display_window: u32,
}

impl RequestContextEstimate {
    pub fn ratio(&self) -> f64 {
        if self.input_limit > 0 {
            self.input_tokens as f64 / self.input_limit as f64
        } else {
            0.0
        }
    }

    pub fn as_usage(&self) -> ContextUsage {
        ContextUsage {
            used: self.input_tokens,
            limit: self.input_limit,
            ratio: self.ratio(),
        }
    }
}

/// Resolved budget used for context display and preflight checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudget {
    /// Provider/model maximum total context when known.
    pub total_window: u32,
    /// Maximum prompt/input tokens accepted by the provider.
    pub input_limit: u32,
    /// Maximum output tokens the model can generate.
    pub output_limit: u32,
    /// User-visible denominator. This may intentionally differ from the hard
    /// input limit so very large windows can be displayed in a stable, readable
    /// way (for example GPT-5.5 displays as 1.0M).
    pub display_window: u32,
    /// Tokens intentionally unavailable to prompt input because they are held
    /// for output or provider recovery room.
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
            input_limit: 922_000,
            output_limit: 128_000,
            display_window: 1_000_000,
            reserved_buffer: 128_000,
        };
    }

    ContextBudget {
        total_window: meta.context_window,
        input_limit: meta.context_window,
        output_limit: meta.max_output_tokens,
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

fn openai_responses_message_items(message: &Message) -> Vec<serde_json::Value> {
    match message {
        Message::User(user) => {
            let has_images = user
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }));
            if has_images {
                let parts: Vec<serde_json::Value> = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(serde_json::json!({
                            "type": "input_text",
                            "text": text,
                        })),
                        ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                            "type": "input_image",
                            "image_url": format!("data:{media_type};base64,{data}"),
                        })),
                        _ => None,
                    })
                    .collect();
                vec![serde_json::json!({ "role": "user", "content": parts })]
            } else {
                let text = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![serde_json::json!({ "role": "user", "content": text })]
            }
        }
        Message::Assistant(assistant) => {
            let mut items = Vec::new();
            let text_parts: Vec<serde_json::Value> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(serde_json::json!({
                        "type": "output_text",
                        "text": text,
                    })),
                    ContentBlock::Thinking { text } => Some(serde_json::json!({
                        "type": "reasoning_text",
                        "text": text,
                    })),
                    _ => None,
                })
                .collect();
            if !text_parts.is_empty() {
                items.push(serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "content": text_parts,
                }));
            }
            for block in &assistant.content {
                if let ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                } = block
                {
                    items.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": arguments.to_string(),
                    }));
                }
            }
            items
        }
        Message::ToolResult(result) => {
            let mut output_parts = Vec::new();
            let mut images_to_inject = Vec::new();
            for block in &result.content {
                match block {
                    ContentBlock::Text { text } => output_parts.push(text.clone()),
                    ContentBlock::Image { media_type, data } => {
                        output_parts.push("[Image attached below]".to_string());
                        images_to_inject.push((media_type.clone(), data.clone()));
                    }
                    _ => {}
                }
            }
            let mut items = vec![serde_json::json!({
                "type": "function_call_output",
                "call_id": result.tool_call_id,
                "output": output_parts.join("\n"),
            })];
            if !images_to_inject.is_empty() {
                let image_parts: Vec<serde_json::Value> = images_to_inject
                    .iter()
                    .map(|(mime, data)| {
                        serde_json::json!({
                            "type": "input_image",
                            "image_url": format!("data:{mime};base64,{data}"),
                        })
                    })
                    .collect();
                items.push(serde_json::json!({ "role": "user", "content": image_parts }));
            }
            items
        }
    }
}

fn estimate_openai_responses_message_tokens_for_model(message: &Message, meta: &ModelMeta) -> u32 {
    let serialized =
        serde_json::to_string(&openai_responses_message_items(message)).unwrap_or_default();
    estimate_text_tokens_for_model(&serialized, meta)
}

/// Estimate one message's contribution to provider request input.
pub fn estimate_message_tokens_for_model(message: &Message, meta: &ModelMeta) -> u32 {
    if model_uses_openai_tokenizer(meta) {
        return estimate_openai_responses_message_tokens_for_model(message, meta);
    }

    let json = serde_json::to_string(message).unwrap_or_default();
    estimate_tokens(&json)
}

fn estimate_openai_responses_tool_tokens_for_model(tool: &ToolDefinition, meta: &ModelMeta) -> u32 {
    let serialized = serde_json::to_string(&serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    }))
    .unwrap_or_default();
    12 + estimate_text_tokens_for_model(&serialized, meta)
}

fn estimate_tool_definition_tokens_for_model(tool: &ToolDefinition, meta: &ModelMeta) -> u32 {
    if model_uses_openai_tokenizer(meta) {
        return estimate_openai_responses_tool_tokens_for_model(tool, meta);
    }
    let serialized = serde_json::to_string(tool).unwrap_or_default();
    12 + estimate_text_tokens_for_model(&serialized, meta)
}

fn estimate_tool_definitions_tokens_for_model(tools: &[ToolDefinition], meta: &ModelMeta) -> u32 {
    // Providers sort tool definitions for prompt-cache stability in several
    // paths. Sorting here does not change the token count for plain text, but
    // it makes diagnostics deterministic and mirrors request construction.
    let mut sorted: Vec<&ToolDefinition> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
        .iter()
        .map(|tool| estimate_tool_definition_tokens_for_model(tool, meta))
        .sum()
}

fn planned_output_tokens(model: &Model, options: &RequestOptions) -> u32 {
    let budget = context_budget(model);
    options
        .max_tokens
        .unwrap_or(budget.output_limit)
        .min(budget.output_limit)
}

/// Estimate the exact provider-bound request candidate.
///
/// Unlike [`context_usage`], this includes request scaffolding that can be large
/// enough to determine whether a provider accepts the request: system prompt,
/// tool schemas, active messages, tool-call arguments/results, images, and the
/// planned output cap used for diagnostics.
pub fn estimate_request_context(
    messages: &[Message],
    model: &Model,
    options: &RequestOptions,
) -> RequestContextEstimate {
    let budget = context_budget(model);
    let system_tokens = if options.system_prompt.is_empty() {
        0
    } else {
        8 + estimate_text_tokens_for_model(&options.system_prompt, &model.meta)
    };
    let tool_tokens = estimate_tool_definitions_tokens_for_model(&options.tools, &model.meta);
    let message_tokens: u32 = messages
        .iter()
        .map(|message| estimate_message_tokens_for_model(message, &model.meta))
        .sum();
    let input_tokens = system_tokens
        .saturating_add(tool_tokens)
        .saturating_add(message_tokens);

    RequestContextEstimate {
        input_tokens,
        system_tokens,
        tool_definition_tokens: tool_tokens,
        message_tokens,
        input_limit: budget.input_limit,
        output_tokens: planned_output_tokens(model, options),
        display_window: budget.display_window,
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
/// tool result content is replaced with a stable digest preserving the tool
/// name, a truncated summary of args, and the byte count. Passing
/// `keep_recent_turns = 0` masks all observed tool results except the latest
/// active tool-call/result pair.
pub fn mask_observations(messages: &mut [Message], keep_recent_turns: usize) {
    let projected = projection::digest_old_tool_results(messages, keep_recent_turns);
    for (message, projected_message) in messages.iter_mut().zip(projected) {
        *message = projected_message;
    }
}

#[cfg(test)]
mod tests;
