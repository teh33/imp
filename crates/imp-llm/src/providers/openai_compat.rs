use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::auth::{ApiKey, AuthStore};
use crate::error::{Error, Result};
use crate::message::{AssistantMessage, ContentBlock, Message, StopReason};
use crate::model::{Model, ModelMeta};
use crate::provider::{Context, Provider, RequestOptions, ToolDefinition};
use crate::stream::StreamEvent;
use crate::usage::Usage;

// ---------------------------------------------------------------------------
// OpenAI Chat Completions wire-format types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    messages: Vec<ApiMessage>,
    stream: bool,
    stream_options: ApiStreamOptions,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<KimiThinking>,
}

#[derive(Debug, Serialize)]
struct KimiThinking {
    #[serde(rename = "type")]
    thinking_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ApiStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
}

#[derive(Debug, Serialize)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiToolCallFunction,
}

#[derive(Debug, Serialize)]
struct ApiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ApiToolDef {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiToolDefFunction,
}

#[derive(Debug, Serialize)]
struct ApiToolDefFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions wire-format types (SSE response)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SseChunk {
    #[serde(default)]
    choices: Vec<SseChoice>,
    #[serde(default)]
    usage: Option<SseUsage>,
}

#[derive(Debug, Deserialize)]
struct SseChoice {
    delta: SseDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek-style reasoning content field.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SseToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct SseToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseToolCallFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct SseToolCallFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// SSE stream state
// ---------------------------------------------------------------------------

struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

// ---------------------------------------------------------------------------
// Provider implementation
// ---------------------------------------------------------------------------

/// OpenAI Chat Completions API provider.
///
/// Used by third-party providers (DeepSeek, Groq, Together, Mistral, xAI,
/// OpenRouter, Fireworks) that expose an OpenAI-compatible `/v1/chat/completions`
/// endpoint.
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    provider_id: String,
    base_url: String,
    models: Vec<ModelMeta>,
    default_headers: reqwest::header::HeaderMap,
}

fn default_max_tokens(model: &Model) -> u32 {
    model.meta.max_output_tokens.min(8_192)
}

impl OpenAiCompatProvider {
    /// Create a new OpenAI-compatible provider.
    ///
    /// - `provider_id`: matches the provider's id in the registry (e.g. "deepseek")
    /// - `base_url`: API root, e.g. "https://api.deepseek.com" (no trailing slash)
    /// - `models`: models this provider can serve
    pub fn new(provider_id: &str, base_url: &str, models: Vec<ModelMeta>) -> Self {
        Self {
            client: super::streaming_http_client(),
            provider_id: provider_id.to_string(),
            base_url: base_url.to_string(),
            models,
            default_headers: reqwest::header::HeaderMap::new(),
        }
    }

    /// Set default headers to send with every request.
    pub fn with_default_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.default_headers = headers;
        self
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn api_model_id(model: &Model) -> &str {
    match (model.meta.provider.as_str(), model.meta.id.as_str()) {
        ("kimi-code", "kimi2.6") => "kimi-k2.6",
        ("kimi-code", "kimi2.7") => "kimi-k2.7-code",
        _ => &model.meta.id,
    }
}

fn build_request(model: &Model, context: Context, options: RequestOptions) -> ApiRequest {
    let mut messages = Vec::new();

    // System prompt becomes a leading system message.
    if !options.system_prompt.is_empty() {
        messages.push(ApiMessage {
            role: "system".into(),
            content: Some(serde_json::Value::String(options.system_prompt.clone())),
            reasoning_content: None,
            tool_call_id: None,
            tool_calls: None,
        });
    }

    for msg in &context.messages {
        messages.extend(convert_message(msg));
    }

    let tools = build_tool_defs(&options.tools);
    let max_tokens = options.max_tokens.or(Some(default_max_tokens(model)));

    ApiRequest {
        model: api_model_id(model).to_string(),
        messages,
        stream: true,
        stream_options: ApiStreamOptions {
            include_usage: true,
        },
        tools,
        temperature: kimi_compatible_temperature(&model.meta, options.temperature),
        max_tokens,
        thinking: kimi_thinking_config(&model.meta, options.thinking_level),
    }
}

fn kimi_behavior_model_id(meta: &ModelMeta) -> &str {
    match (meta.provider.as_str(), meta.id.as_str()) {
        ("kimi-code", "kimi2.6") => "kimi-k2.6",
        ("kimi-code", "kimi2.7") => "kimi-k2.7-code",
        _ => &meta.id,
    }
}

fn is_kimi_configurable_thinking_model(model_id: &str) -> bool {
    matches!(model_id, "kimi-k2.6" | "kimi-k2.5")
}

fn is_kimi_forced_thinking_model(model_id: &str) -> bool {
    matches!(
        model_id,
        "kimi-k2-thinking"
            | "kimi-k2-thinking-turbo"
            | "kimi-k2.7-code"
            | "kimi-k2.7-code-highspeed"
    )
}

fn is_kimi_fixed_temperature_model(model_id: &str) -> bool {
    is_kimi_configurable_thinking_model(model_id) || is_kimi_forced_thinking_model(model_id)
}

fn kimi_compatible_temperature(meta: &ModelMeta, temperature: Option<f32>) -> Option<f32> {
    if is_kimi_fixed_temperature_model(kimi_behavior_model_id(meta)) {
        None
    } else {
        temperature
    }
}

fn kimi_thinking_config(
    meta: &ModelMeta,
    level: crate::provider::ThinkingLevel,
) -> Option<KimiThinking> {
    let model_id = kimi_behavior_model_id(meta);
    if is_kimi_forced_thinking_model(model_id) {
        return Some(KimiThinking {
            thinking_type: "enabled",
            keep: Some("all"),
        });
    }

    if !is_kimi_configurable_thinking_model(model_id) {
        return None;
    }

    match level {
        crate::provider::ThinkingLevel::Off | crate::provider::ThinkingLevel::Minimal => {
            Some(KimiThinking {
                thinking_type: "disabled",
                keep: None,
            })
        }
        crate::provider::ThinkingLevel::Low
        | crate::provider::ThinkingLevel::Medium
        | crate::provider::ThinkingLevel::High
        | crate::provider::ThinkingLevel::XHigh => Some(KimiThinking {
            thinking_type: "enabled",
            keep: Some("all"),
        }),
    }
}

fn build_tool_defs(tools: &[ToolDefinition]) -> Vec<ApiToolDef> {
    tools
        .iter()
        .map(|t| ApiToolDef {
            tool_type: "function".into(),
            function: ApiToolDefFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        })
        .collect()
}

/// Convert an internal Message to one or more Chat Completions API messages.
fn convert_message(msg: &Message) -> Vec<ApiMessage> {
    match msg {
        Message::User(u) => {
            let has_images = u
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }));

            let content = if has_images {
                // Content array with text + image_url items.
                let parts: Vec<serde_json::Value> = u
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(serde_json::json!({
                            "type": "text",
                            "text": text
                        })),
                        ContentBlock::Image { media_type, data } => Some(serde_json::json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{media_type};base64,{data}") }
                        })),
                        _ => None,
                    })
                    .collect();
                serde_json::Value::Array(parts)
            } else {
                let text: String = u
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                serde_json::Value::String(text)
            };

            vec![ApiMessage {
                role: "user".into(),
                content: Some(content),
                reasoning_content: None,
                tool_call_id: None,
                tool_calls: None,
            }]
        }
        Message::Assistant(a) => {
            let text: String = a
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            let reasoning_content = a
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Thinking { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            let tool_calls: Vec<ApiToolCall> = a
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some(ApiToolCall {
                        id: id.clone(),
                        call_type: "function".into(),
                        function: ApiToolCallFunction {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                    }),
                    _ => None,
                })
                .collect();

            let content = if text.is_empty() {
                None
            } else {
                Some(serde_json::Value::String(text))
            };
            let tool_calls_opt = if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            };

            vec![ApiMessage {
                role: "assistant".into(),
                content,
                reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
                tool_call_id: None,
                tool_calls: tool_calls_opt,
            }]
        }
        Message::ToolResult(tr) => {
            let output: String = tr
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            vec![ApiMessage {
                role: "tool".into(),
                content: Some(serde_json::Value::String(output)),
                reasoning_content: None,
                tool_call_id: Some(tr.tool_call_id.clone()),
                tool_calls: None,
            }]
        }
    }
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

fn sse_data_payload(line: &str) -> Option<&str> {
    line.strip_prefix("data:").map(str::trim_start)
}

fn parse_sse_chunk(data: &str) -> Result<Option<SseChunk>> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|e| Error::Stream(format!("Failed to parse SSE chunk: {e}: {trimmed}")))
}

// ---------------------------------------------------------------------------
// Streaming implementation
// ---------------------------------------------------------------------------

fn stream_response(
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    request: ApiRequest,
    default_headers: reqwest::header::HeaderMap,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = futures::channel::mpsc::unbounded();

    tokio::spawn(async move {
        let url = format!("{base_url}/v1/chat/completions");

        let result = client
            .post(&url)
            .headers(default_headers)
            .bearer_auth(&api_key)
            .json(&request)
            .send()
            .await;

        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.unbounded_send(Err(Error::Http(e)));
                return;
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body =
                crate::auth::redact_provider_error_body(&resp.text().await.unwrap_or_default());
            let _ = tx.unbounded_send(Err(Error::Provider(format!("HTTP {status}: {body}"))));
            return;
        }

        // Emit MessageStart once we have a successful response. The model id
        // comes from the request since Chat Completions doesn't include it in
        // every SSE chunk (unlike Anthropic).
        if tx
            .unbounded_send(Ok(StreamEvent::MessageStart {
                model: request.model.clone(),
            }))
            .is_err()
        {
            return;
        }

        let mut tool_accum: HashMap<usize, ToolCallAccum> = HashMap::new();
        let mut content_buf = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::EndTurn;
        let mut buf = String::new();
        let mut saw_finish_reason = false;

        use futures::StreamExt;
        let mut byte_stream = resp.bytes_stream();

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf = buf[pos + 1..].to_string();

                        let trimmed = line.trim();
                        if let Some(data) = sse_data_payload(trimmed) {
                            match parse_sse_chunk(data) {
                                Ok(Some(chunk)) => {
                                    if let Some(u) = chunk.usage {
                                        usage.input_tokens = u.prompt_tokens;
                                        usage.output_tokens = u.completion_tokens;
                                    }

                                    for choice in chunk.choices {
                                        let delta = choice.delta;

                                        // Thinking/reasoning content (DeepSeek, Kimi, etc.)
                                        if let Some(reasoning) = delta.reasoning_content {
                                            if !reasoning.is_empty() {
                                                content_buf.push(ContentBlock::Thinking {
                                                    text: reasoning.clone(),
                                                });
                                                if tx
                                                    .unbounded_send(Ok(
                                                        StreamEvent::ThinkingDelta {
                                                            text: reasoning,
                                                        },
                                                    ))
                                                    .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                        }

                                        // Regular text content
                                        if let Some(text) = delta.content {
                                            if !text.is_empty() {
                                                content_buf.push(ContentBlock::Text {
                                                    text: text.clone(),
                                                });
                                                if tx
                                                    .unbounded_send(Ok(StreamEvent::TextDelta {
                                                        text,
                                                    }))
                                                    .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                        }

                                        // Tool call deltas
                                        if let Some(tc_deltas) = delta.tool_calls {
                                            for tc in tc_deltas {
                                                let entry = tool_accum
                                                    .entry(tc.index)
                                                    .or_insert_with(|| ToolCallAccum {
                                                        id: String::new(),
                                                        name: String::new(),
                                                        arguments: String::new(),
                                                    });
                                                if let Some(id) = tc.id {
                                                    entry.id = id;
                                                }
                                                if let Some(func) = tc.function {
                                                    if let Some(name) = func.name {
                                                        entry.name = name;
                                                    }
                                                    if let Some(args) = func.arguments {
                                                        entry.arguments.push_str(&args);
                                                    }
                                                }
                                            }
                                        }

                                        // Finish reason
                                        if let Some(reason) = choice.finish_reason {
                                            saw_finish_reason = true;
                                            stop_reason = match reason.as_str() {
                                                "stop" => StopReason::EndTurn,
                                                "tool_calls" => StopReason::ToolUse,
                                                "length" => StopReason::MaxTokens,
                                                other => StopReason::Error(other.to_string()),
                                            };
                                        }
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    let _ = tx.unbounded_send(Err(e));
                                    return;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.unbounded_send(Err(Error::Http(e)));
                    return;
                }
            }
        }

        let trimmed = buf.trim();
        if let Some(data) = sse_data_payload(trimmed) {
            match parse_sse_chunk(data) {
                Ok(Some(chunk)) => {
                    if let Some(u) = chunk.usage {
                        usage.input_tokens = u.prompt_tokens;
                        usage.output_tokens = u.completion_tokens;
                    }

                    for choice in chunk.choices {
                        let delta = choice.delta;

                        if let Some(reasoning) = delta.reasoning_content {
                            if !reasoning.is_empty() {
                                content_buf.push(ContentBlock::Thinking {
                                    text: reasoning.clone(),
                                });
                                if tx
                                    .unbounded_send(Ok(StreamEvent::ThinkingDelta {
                                        text: reasoning,
                                    }))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }

                        if let Some(text) = delta.content {
                            if !text.is_empty() {
                                content_buf.push(ContentBlock::Text { text: text.clone() });
                                if tx
                                    .unbounded_send(Ok(StreamEvent::TextDelta { text }))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }

                        if let Some(tc_deltas) = delta.tool_calls {
                            for tc in tc_deltas {
                                let entry =
                                    tool_accum.entry(tc.index).or_insert_with(|| ToolCallAccum {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                if let Some(id) = tc.id {
                                    entry.id = id;
                                }
                                if let Some(func) = tc.function {
                                    if let Some(name) = func.name {
                                        entry.name = name;
                                    }
                                    if let Some(args) = func.arguments {
                                        entry.arguments.push_str(&args);
                                    }
                                }
                            }
                        }

                        if let Some(reason) = choice.finish_reason {
                            saw_finish_reason = true;
                            stop_reason = match reason.as_str() {
                                "stop" => StopReason::EndTurn,
                                "tool_calls" => StopReason::ToolUse,
                                "length" => StopReason::MaxTokens,
                                other => StopReason::Error(other.to_string()),
                            };
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = tx.unbounded_send(Err(e));
                    return;
                }
            }
        }

        // Some OpenAI-compatible providers (e.g. Kimi Code) don't always send a
        // finish_reason in the last SSE chunk. If we received content and the
        // stream ended cleanly, treat it as a normal end-of-turn rather than
        // an error.
        if !saw_finish_reason {
            stop_reason = StopReason::EndTurn;
        }

        // Emit complete tool calls after stream ends.
        let mut tc_indices: Vec<usize> = tool_accum.keys().copied().collect();
        tc_indices.sort();
        for idx in tc_indices {
            if let Some(tc) = tool_accum.remove(&idx) {
                let arguments: serde_json::Value = serde_json::from_str(&tc.arguments)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                content_buf.push(ContentBlock::ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: arguments.clone(),
                });
                if tx
                    .unbounded_send(Ok(StreamEvent::ToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments,
                    }))
                    .is_err()
                {
                    return;
                }
            }
        }

        let message = AssistantMessage {
            content: content_buf,
            usage: Some(usage),
            stop_reason,
            timestamp: crate::now(),
        };
        let _ = tx.unbounded_send(Ok(StreamEvent::MessageEnd { message }));
    });

    Box::pin(rx)
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn stream(
        &self,
        model: &Model,
        context: Context,
        options: RequestOptions,
        api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
        let request = build_request(model, context, options);
        stream_response(
            self.client.clone(),
            self.base_url.clone(),
            api_key.to_string(),
            request,
            self.default_headers.clone(),
        )
    }

    async fn resolve_auth(&self, auth: &AuthStore) -> Result<ApiKey> {
        auth.resolve(&self.provider_id)
    }

    fn id(&self) -> &str {
        &self.provider_id
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "openai_compat/tests.rs"]
mod tests;
