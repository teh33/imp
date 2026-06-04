use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::auth::{ApiKey, AuthStore};
use crate::error::{Error, Result};
use crate::message::{AssistantMessage, ContentBlock, Message, StopReason, ToolResultMessage};
use crate::model::{Capabilities, Model, ModelMeta, ModelPricing};
use crate::provider::{Context, Provider, RequestOptions, ThinkingLevel, ToolDefinition};
use crate::stream::StreamEvent;
use crate::usage::Usage;

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const FLASH_MAX_THINKING_BUDGET: i32 = 24_576;
const PRO_MAX_THINKING_BUDGET: i32 = 32_768;

// ---------------------------------------------------------------------------
// Gemini wire-format types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ApiRequest {
    contents: Vec<ApiContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<ApiInstruction>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(rename = "generationConfig")]
    generation_config: ApiGenerationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiInstruction {
    parts: Vec<ApiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiContent {
    role: String,
    parts: Vec<ApiPart>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ApiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    function_call: Option<ApiFunctionCall>,
    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    function_response: Option<ApiFunctionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunctionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ApiTool {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<ApiFunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ApiGenerationConfig {
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ApiThinkingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ApiThinkingConfig {
    #[serde(rename = "includeThoughts")]
    include_thoughts: bool,
    #[serde(rename = "thinkingBudget")]
    thinking_budget: i32,
}

// ---------------------------------------------------------------------------
// Gemini wire-format types (SSE response)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ApiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<ApiUsageMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiCandidate {
    content: Option<ApiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u32,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u32,
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: u32,
    #[serde(rename = "cachedContentTokenCount", default)]
    cached_content_token_count: u32,
}

// ---------------------------------------------------------------------------
// SSE stream state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum PartState {
    Text(String),
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        emitted: bool,
    },
}

#[derive(Debug)]
struct StreamState {
    model: String,
    started: bool,
    finished: bool,
    parts: Vec<PartState>,
    usage: Usage,
    finish_reason: Option<String>,
    saw_tool_call: bool,
}

impl StreamState {
    fn new(model: String) -> Self {
        Self {
            model,
            started: false,
            finished: false,
            parts: Vec::new(),
            usage: Usage::default(),
            finish_reason: None,
            saw_tool_call: false,
        }
    }

    fn ensure_index(&mut self, index: usize) {
        while self.parts.len() <= index {
            self.parts.push(PartState::Text(String::new()));
        }
    }

    fn stop_reason(&self) -> StopReason {
        if self.saw_tool_call {
            return StopReason::ToolUse;
        }

        match self.finish_reason.as_deref() {
            Some("STOP") | Some("FINISH_REASON_UNSPECIFIED") | None => StopReason::EndTurn,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            Some(other) => StopReason::Error(other.to_string()),
        }
    }

    fn build_message(&self) -> AssistantMessage {
        let content = self
            .parts
            .iter()
            .filter_map(|part| match part {
                PartState::Text(text) if !text.is_empty() => {
                    Some(ContentBlock::Text { text: text.clone() })
                }
                PartState::Thinking(text) if !text.is_empty() => {
                    Some(ContentBlock::Thinking { text: text.clone() })
                }
                PartState::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some(ContentBlock::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                }),
                _ => None,
            })
            .collect();

        AssistantMessage {
            content,
            usage: Some(self.usage.clone()),
            stop_reason: self.stop_reason(),
            timestamp: crate::now(),
        }
    }
}

/// Google Gemini API provider.
pub struct GoogleProvider {
    client: reqwest::Client,
    models: Vec<ModelMeta>,
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self {
            client: super::streaming_http_client(),
            models: builtin_models(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn max_thinking_budget(model_id: &str) -> i32 {
    if model_id.contains("flash") {
        FLASH_MAX_THINKING_BUDGET
    } else {
        PRO_MAX_THINKING_BUDGET
    }
}

fn thinking_budget(model: &Model, level: ThinkingLevel) -> Option<i32> {
    let budget = match level {
        ThinkingLevel::Off => return None,
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 4096,
        ThinkingLevel::Medium => 10_000,
        ThinkingLevel::High => 24_576,
        ThinkingLevel::XHigh => max_thinking_budget(&model.meta.id),
    };

    Some(budget.min(max_thinking_budget(&model.meta.id)))
}

fn default_max_output_tokens(model: &Model, thinking_budget: Option<i32>) -> u32 {
    let base = model.meta.max_output_tokens.min(8_192);
    match thinking_budget {
        Some(budget) => base.max((budget as u32).saturating_add(1024)),
        None => base,
    }
}

fn build_request(model: &Model, context: Context, options: RequestOptions) -> ApiRequest {
    let thinking_config =
        thinking_budget(model, options.thinking_level).map(|thinking_budget| ApiThinkingConfig {
            include_thoughts: true,
            thinking_budget,
        });

    ApiRequest {
        contents: build_messages(&context.messages),
        system_instruction: build_system_instruction(&options.system_prompt),
        tools: build_tools(&options.tools),
        generation_config: ApiGenerationConfig {
            max_output_tokens: options.max_tokens.or(Some(default_max_output_tokens(
                model,
                thinking_budget(model, options.thinking_level),
            ))),
            temperature: options.temperature,
            thinking_config,
        },
    }
}

fn build_system_instruction(prompt: &str) -> Option<ApiInstruction> {
    if prompt.is_empty() {
        return None;
    }

    Some(ApiInstruction {
        parts: vec![ApiPart {
            text: Some(prompt.to_string()),
            ..Default::default()
        }],
    })
}

fn build_tools(tools: &[ToolDefinition]) -> Vec<ApiTool> {
    if tools.is_empty() {
        return Vec::new();
    }

    vec![ApiTool {
        function_declarations: tools.iter().map(convert_tool_def).collect(),
    }]
}

fn build_messages(messages: &[Message]) -> Vec<ApiContent> {
    messages.iter().map(convert_message).collect()
}

fn convert_message(message: &Message) -> ApiContent {
    match message {
        Message::User(user) => ApiContent {
            role: "user".into(),
            parts: user
                .content
                .iter()
                .filter_map(convert_content_block)
                .collect(),
        },
        Message::Assistant(assistant) => ApiContent {
            role: "model".into(),
            parts: assistant
                .content
                .iter()
                .filter_map(convert_content_block)
                .collect(),
        },
        Message::ToolResult(tool_result) => ApiContent {
            role: "user".into(),
            parts: vec![ApiPart {
                function_response: Some(ApiFunctionResponse {
                    id: Some(tool_result.tool_call_id.clone()),
                    name: tool_result.tool_name.clone(),
                    response: convert_tool_result_response(tool_result),
                }),
                ..Default::default()
            }],
        },
    }
}

fn convert_content_block(block: &ContentBlock) -> Option<ApiPart> {
    match block {
        ContentBlock::Text { text } => Some(ApiPart {
            text: Some(text.clone()),
            ..Default::default()
        }),
        ContentBlock::Thinking { text } => Some(ApiPart {
            text: Some(text.clone()),
            thought: Some(true),
            ..Default::default()
        }),
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
        } => Some(ApiPart {
            function_call: Some(ApiFunctionCall {
                id: Some(id.clone()),
                name: name.clone(),
                args: arguments.clone(),
            }),
            ..Default::default()
        }),
        ContentBlock::Image { .. } => None,
    }
}

fn convert_tool_result_response(tool_result: &ToolResultMessage) -> serde_json::Value {
    let output = tool_result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut response = serde_json::Map::new();
    response.insert("result".into(), serde_json::Value::String(output));

    if tool_result.is_error {
        response.insert("isError".into(), serde_json::Value::Bool(true));
    }

    if !tool_result.details.is_null() {
        response.insert("details".into(), tool_result.details.clone());
    }

    serde_json::Value::Object(response)
}

fn convert_tool_def(tool: &ToolDefinition) -> ApiFunctionDeclaration {
    ApiFunctionDeclaration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.parameters.clone(),
    }
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

fn parse_sse_event(data: &str) -> Result<Option<GenerateContentResponse>> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }

    serde_json::from_str(trimmed)
        .map(Some)
        .map_err(|e| Error::Stream(format!("Failed to parse Gemini SSE data: {e}: {trimmed}")))
}

fn text_delta(previous: &str, current: &str) -> String {
    current
        .strip_prefix(previous)
        .unwrap_or(current)
        .to_string()
}

fn update_usage(usage: &ApiUsageMetadata, state: &mut StreamState) {
    state.usage.input_tokens = usage.prompt_token_count;
    state.usage.output_tokens = usage.candidates_token_count + usage.thoughts_token_count;
    state.usage.cache_read_tokens = usage.cached_content_token_count;
    state.usage.cache_write_tokens = 0;
}

fn process_response(
    response: GenerateContentResponse,
    state: &mut StreamState,
) -> Vec<StreamEvent> {
    let mut out = Vec::new();

    if !state.started {
        state.started = true;
        out.push(StreamEvent::MessageStart {
            model: state.model.clone(),
        });
    }

    if let Some(usage) = &response.usage_metadata {
        update_usage(usage, state);
    }

    if let Some(candidate) = response.candidates.first() {
        if let Some(content) = &candidate.content {
            for (index, part) in content.parts.iter().enumerate() {
                if let Some(function_call) = &part.function_call {
                    state.ensure_index(index);
                    let id = function_call
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("call_{index}"));
                    let name = function_call.name.clone();
                    let arguments = function_call.args.clone();

                    let emit = match state.parts.get_mut(index) {
                        Some(PartState::ToolCall {
                            id: existing_id,
                            name: existing_name,
                            arguments: existing_arguments,
                            emitted,
                        }) if *existing_id == id && *existing_name == name => {
                            *existing_arguments = arguments.clone();
                            if *emitted {
                                false
                            } else {
                                *emitted = true;
                                true
                            }
                        }
                        Some(slot) => {
                            *slot = PartState::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                emitted: true,
                            };
                            true
                        }
                        None => false,
                    };

                    state.saw_tool_call = true;
                    if emit {
                        out.push(StreamEvent::ToolCall {
                            id,
                            name,
                            arguments,
                        });
                    }
                    continue;
                }

                let Some(text) = part.text.as_deref() else {
                    continue;
                };

                state.ensure_index(index);
                if part.thought.unwrap_or(false) {
                    let previous = match &state.parts[index] {
                        PartState::Thinking(existing) => existing.clone(),
                        _ => String::new(),
                    };
                    state.parts[index] = PartState::Thinking(text.to_string());
                    let delta = text_delta(&previous, text);
                    if !delta.is_empty() {
                        out.push(StreamEvent::ThinkingDelta { text: delta });
                    }
                } else {
                    let previous = match &state.parts[index] {
                        PartState::Text(existing) => existing.clone(),
                        _ => String::new(),
                    };
                    state.parts[index] = PartState::Text(text.to_string());
                    let delta = text_delta(&previous, text);
                    if !delta.is_empty() {
                        out.push(StreamEvent::TextDelta { text: delta });
                    }
                }
            }
        }

        if let Some(reason) = &candidate.finish_reason {
            state.finish_reason = Some(reason.clone());
        }

        if candidate.finish_reason.is_some() && !state.finished {
            state.finished = true;
            out.push(StreamEvent::MessageEnd {
                message: state.build_message(),
            });
        }
    }

    out
}

#[cfg(test)]
fn parse_sse_stream(raw: &str, state: &mut StreamState) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(data) = trimmed.strip_prefix("data: ") {
            match parse_sse_event(data) {
                Ok(Some(response)) => {
                    for event in process_response(response, state) {
                        events.push(Ok(event));
                    }
                }
                Ok(None) => {}
                Err(error) => events.push(Err(error)),
            }
        }
    }

    events
}

// ---------------------------------------------------------------------------
// Streaming implementation
// ---------------------------------------------------------------------------

fn stream_response(
    client: reqwest::Client,
    model_id: String,
    api_key: String,
    request: ApiRequest,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = futures::channel::mpsc::unbounded();

    tokio::spawn(async move {
        let result = client
            .post(format!("{API_BASE_URL}/{model_id}:streamGenerateContent"))
            .query(&[("alt", "sse"), ("key", api_key.as_str())])
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await;

        let response = match result {
            Ok(response) => response,
            Err(error) => {
                let _ = tx.unbounded_send(Err(Error::Http(error)));
                return;
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body =
                crate::auth::redact_provider_error_body(&response.text().await.unwrap_or_default());
            let _ = tx.unbounded_send(Err(Error::Provider(format!("HTTP {status}: {body}"))));
            return;
        }

        let mut state = StreamState::new(model_id);
        let mut buffer = String::new();
        let mut byte_stream = response.bytes_stream();

        use futures::StreamExt;
        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer[..pos].to_string();
                        buffer = buffer[pos + 1..].to_string();

                        let trimmed = line.trim();
                        if let Some(data) = trimmed.strip_prefix("data: ") {
                            match parse_sse_event(data) {
                                Ok(Some(response)) => {
                                    for event in process_response(response, &mut state) {
                                        if tx.unbounded_send(Ok(event)).is_err() {
                                            return;
                                        }
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    if tx.unbounded_send(Err(error)).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.unbounded_send(Err(Error::Http(error)));
                    return;
                }
            }
        }

        let trimmed = buffer.trim();
        if let Some(data) = trimmed.strip_prefix("data: ") {
            match parse_sse_event(data) {
                Ok(Some(response)) => {
                    for event in process_response(response, &mut state) {
                        if tx.unbounded_send(Ok(event)).is_err() {
                            return;
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = tx.unbounded_send(Err(error));
                    return;
                }
            }
        }

        if !state.finished {
            let _ = tx.unbounded_send(Err(Error::Stream(
                "Google stream ended before terminal finishReason".into(),
            )));
        }
    });

    Box::pin(rx)
}

#[async_trait]
impl Provider for GoogleProvider {
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
            model.meta.id.clone(),
            api_key.to_string(),
            request,
        )
    }

    async fn resolve_auth(&self, auth: &AuthStore) -> Result<ApiKey> {
        auth.resolve("google")
    }

    fn id(&self) -> &str {
        "google"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

fn builtin_models() -> Vec<ModelMeta> {
    vec![
        ModelMeta {
            id: "gemini-2.5-pro".into(),
            provider: "google".into(),
            name: "Gemini 2.5 Pro".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing {
                input_per_mtok: 1.25,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 0.315,
                cache_write_per_mtok: 1.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gemini-2.5-flash".into(),
            provider: "google".into(),
            name: "Gemini 2.5 Flash".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing {
                input_per_mtok: 0.15,
                output_per_mtok: 3.5,
                cache_read_per_mtok: 0.0375,
                cache_write_per_mtok: 0.15,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
    ]
}

#[cfg(test)]
#[path = "google/tests.rs"]
mod tests;
