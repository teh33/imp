use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::auth::{ApiKey, AuthStore};
use crate::error::{Error, Result};
use crate::message::{AssistantMessage, ContentBlock, Message, StopReason};
use crate::model::{Model, ModelMeta};
use crate::provider::{
    CacheOptions, Context, EffortLevel, Provider, RequestOptions, RetryPolicy, ThinkingLevel,
    ToolDefinition,
};
use crate::stream::StreamEvent;
use crate::usage::Usage;

mod models;

use models::builtin_models;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Anthropic wire-format types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<ApiContentBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ApiThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<ApiOutputConfig>,
}

#[derive(Debug, Serialize)]
struct ApiOutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Vec<ApiContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheControl {
    #[serde(rename = "type")]
    cache_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

fn ephemeral_cache() -> Option<CacheControl> {
    Some(CacheControl {
        cache_type: "ephemeral".into(),
        ttl: None,
        scope: None,
    })
}

fn make_cache_control(options: &CacheOptions) -> Option<CacheControl> {
    Some(CacheControl {
        cache_type: "ephemeral".into(),
        ttl: if options.extended_ttl {
            Some("1h".into())
        } else {
            None
        },
        scope: if options.global_scope {
            Some("global".into())
        } else {
            None
        },
    })
}

#[derive(Debug, Serialize)]
struct ApiToolDef {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ApiThinking {
    #[serde(rename = "enabled")]
    Enabled { budget_tokens: u32 },
    #[serde(rename = "adaptive")]
    Adaptive,
}

// ---------------------------------------------------------------------------
// Anthropic wire-format types (SSE response)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: SseMessage },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: SseContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: SseDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: SseMessageDelta,
        usage: Option<SseUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: SseError },
    /// Catch-all for unknown event types (forward compatibility).
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct SseMessage {
    model: Option<String>,
    usage: Option<SseUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SseContentBlock {
    #[serde(rename = "text")]
    Text {
        #[allow(dead_code)]
        text: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[allow(dead_code)]
        input: Option<serde_json::Value>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[allow(dead_code)]
        thinking: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum SseDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    /// Signature delta (model output verification) — safe to ignore.
    #[serde(rename = "signature_delta")]
    SignatureDelta {
        #[allow(dead_code)]
        signature: String,
    },
    /// Catch-all for future delta types.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct SseMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SseUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct SseError {
    message: String,
}

// ---------------------------------------------------------------------------
// Non-streaming response types (for fallback when streaming fails mid-response)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ApiResponse {
    model: String,
    content: Vec<ApiResponseBlock>,
    stop_reason: Option<String>,
    usage: SseUsage,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ApiResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// Convert a non-streaming API response into the sequence of StreamEvents
/// that the caller would have received from a streaming response.
#[allow(dead_code)]
pub(crate) fn non_streaming_response_to_events(resp: ApiResponse) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    events.push(StreamEvent::MessageStart { model: resp.model });

    let mut content_blocks = Vec::new();
    for block in &resp.content {
        match block {
            ApiResponseBlock::Text { text } => {
                events.push(StreamEvent::TextDelta { text: text.clone() });
                content_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            ApiResponseBlock::Thinking { thinking } => {
                events.push(StreamEvent::ThinkingDelta {
                    text: thinking.clone(),
                });
                content_blocks.push(ContentBlock::Thinking {
                    text: thinking.clone(),
                });
            }
            ApiResponseBlock::ToolUse { id, name, input } => {
                events.push(StreamEvent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                });
                content_blocks.push(ContentBlock::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                });
            }
        }
    }

    let stop_reason = match resp.stop_reason.as_deref() {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some(other) => StopReason::Error(other.to_string()),
        None => StopReason::EndTurn,
    };

    let usage = Usage {
        input_tokens: resp.usage.input_tokens,
        output_tokens: resp.usage.output_tokens,
        cache_read_tokens: resp.usage.cache_read_input_tokens,
        cache_write_tokens: resp.usage.cache_creation_input_tokens,
    };

    events.push(StreamEvent::MessageEnd {
        message: AssistantMessage {
            content: content_blocks,
            usage: Some(usage),
            stop_reason,
            timestamp: crate::now(),
        },
    });

    events
}

// ---------------------------------------------------------------------------
// SSE stream state
// ---------------------------------------------------------------------------

/// Accumulated state for an in-flight content block.
#[derive(Debug)]
enum BlockState {
    Text,
    Thinking,
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
}

/// Tracks the SSE stream so we can assemble a final AssistantMessage.
struct StreamState {
    model: String,
    blocks: Vec<BlockState>,
    content: Vec<ContentBlock>,
    usage: Usage,
    stop_reason: StopReason,
    finished: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            model: String::new(),
            blocks: Vec::new(),
            content: Vec::new(),
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
            finished: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider implementation
// ---------------------------------------------------------------------------

/// Anthropic Messages API provider with streaming SSE support.
pub struct AnthropicProvider {
    client: reqwest::Client,
    retry_policy: RetryPolicy,
    models: Vec<ModelMeta>,
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicProvider {
    pub fn new() -> Self {
        let client = super::streaming_http_client();

        Self {
            client,
            retry_policy: RetryPolicy::default(),
            models: builtin_models(),
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn thinking_budget(level: ThinkingLevel) -> Option<u32> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(1024),
        ThinkingLevel::Low => Some(4096),
        ThinkingLevel::Medium => Some(10_000),
        ThinkingLevel::High => Some(32_000),
        ThinkingLevel::XHigh => Some(100_000),
    }
}

fn default_max_tokens(model: &Model, thinking_budget: Option<u32>, adaptive: bool) -> u32 {
    if adaptive {
        return model.meta.max_output_tokens;
    }

    // Anthropic is much happier when we do not default every request to the
    // model's absolute max output size, especially on larger Opus models.
    // Use a moderate default and only scale up when explicit thinking budgets
    // require it.
    let base = model.meta.max_output_tokens.min(8_192);
    match thinking_budget {
        Some(budget) => base.max(budget.saturating_add(1024)),
        None => base,
    }
}

fn model_supports_adaptive(model_id: &str) -> bool {
    model_id.contains("4-6") || model_id.contains("4.6")
}

fn beta_headers(model: &ModelMeta, effort: Option<EffortLevel>) -> Vec<&'static str> {
    let mut betas = vec![
        "interleaved-thinking-2025-05-14",
        "prompt-caching-scope-2026-01-05",
    ];

    if model.context_window > 200_000 {
        betas.push("context-1m-2025-08-07");
    }

    if effort.is_some() {
        betas.push("effort-2025-11-24");
    }

    betas
}

fn build_request(model: &Model, context: Context, options: RequestOptions) -> ApiRequest {
    let budget = thinking_budget(options.thinking_level);
    let supports_adaptive = model_supports_adaptive(&model.meta.id);
    let adaptive = supports_adaptive
        && matches!(
            options.thinking_level,
            ThinkingLevel::Medium | ThinkingLevel::High | ThinkingLevel::XHigh
        );

    let thinking = match budget {
        None => None,
        Some(_) if adaptive => Some(ApiThinking::Adaptive),
        Some(b) => Some(ApiThinking::Enabled { budget_tokens: b }),
    };

    // max_tokens: use explicit value, or a provider-tuned default, ensuring it
    // exceeds the requested thinking budget.
    let mut max_tokens = options
        .max_tokens
        .unwrap_or_else(|| default_max_tokens(model, budget, adaptive));
    if let Some(b) = budget {
        if !adaptive && max_tokens <= b {
            max_tokens = b + 1024;
        }
    }

    let system = build_system_blocks(&options.system_prompt, &options.cache_options);
    let tools = build_tool_defs(&options.tools, &options.cache_options);
    let messages = build_messages(&context.messages, &options.cache_options);

    // Temperature must not be set when thinking is enabled
    let temperature = if thinking.is_some() {
        None
    } else {
        options.temperature
    };

    let output_config = options.effort.map(|e| ApiOutputConfig {
        effort: Some(match e {
            EffortLevel::Low => "low".into(),
            EffortLevel::Medium => "medium".into(),
            EffortLevel::High => "high".into(),
        }),
    });

    ApiRequest {
        model: model.meta.id.clone(),
        max_tokens,
        messages,
        stream: true,
        system,
        tools,
        temperature,
        thinking,
        output_config,
    }
}

fn build_system_blocks(prompt: &str, cache: &CacheOptions) -> Vec<ApiContentBlock> {
    if prompt.is_empty() {
        return Vec::new();
    }
    vec![ApiContentBlock::Text {
        text: prompt.to_string(),
        cache_control: if cache.cache_system_prompt {
            ephemeral_cache()
        } else {
            None
        },
    }]
}

fn build_tool_defs(tools: &[ToolDefinition], cache: &CacheOptions) -> Vec<ApiToolDef> {
    // Sort tools alphabetically for prompt cache stability — prevents cache
    // busts when tools are registered in different orders between requests.
    let mut sorted: Vec<&ToolDefinition> = tools.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let len = sorted.len();
    sorted
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // Place cache breakpoint on the last tool definition
            let cc = if cache.cache_tools && i == len - 1 {
                make_cache_control(cache)
            } else {
                None
            };
            ApiToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
                cache_control: cc,
            }
        })
        .collect()
}

fn build_messages(messages: &[Message], cache: &CacheOptions) -> Vec<ApiMessage> {
    let mut api_msgs: Vec<ApiMessage> = messages.iter().map(convert_message).collect();

    // Place cache breakpoints on the last N user-turn messages
    if cache.cache_recent_turns > 0 {
        let mut turns_tagged = 0;
        for msg in api_msgs.iter_mut().rev() {
            if msg.role == "user" {
                if let Some(last) = msg.content.last_mut() {
                    set_cache_control(last);
                }
                turns_tagged += 1;
                if turns_tagged >= cache.cache_recent_turns {
                    break;
                }
            }
        }
    }

    api_msgs
}

fn set_cache_control(block: &mut ApiContentBlock) {
    match block {
        ApiContentBlock::Text {
            ref mut cache_control,
            ..
        } => {
            *cache_control = ephemeral_cache();
        }
        ApiContentBlock::ToolResult { .. }
        | ApiContentBlock::Image { .. }
        | ApiContentBlock::ToolUse { .. }
        | ApiContentBlock::Thinking { .. } => {
            // Cache control not applicable to these block types in this context
        }
    }
}

fn convert_message(msg: &Message) -> ApiMessage {
    match msg {
        Message::User(u) => ApiMessage {
            role: "user".into(),
            content: u.content.iter().map(convert_content_block).collect(),
        },
        Message::Assistant(a) => ApiMessage {
            role: "assistant".into(),
            content: a.content.iter().map(convert_content_block).collect(),
        },
        Message::ToolResult(tr) => ApiMessage {
            role: "user".into(),
            content: vec![ApiContentBlock::ToolResult {
                tool_use_id: tr.tool_call_id.clone(),
                content: tr.content.iter().map(convert_content_block).collect(),
                is_error: if tr.is_error { Some(true) } else { None },
            }],
        },
    }
}

fn convert_content_block(block: &ContentBlock) -> ApiContentBlock {
    match block {
        ContentBlock::Text { text } => ApiContentBlock::Text {
            text: text.clone(),
            cache_control: None,
        },
        ContentBlock::Thinking { text } => ApiContentBlock::Thinking {
            thinking: text.clone(),
        },
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
        } => ApiContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: arguments.clone(),
        },
        ContentBlock::Image { media_type, data } => ApiContentBlock::Image {
            source: ImageSource {
                source_type: "base64".into(),
                media_type: media_type.clone(),
                data: data.clone(),
            },
        },
    }
}

// ---------------------------------------------------------------------------
// Tool definitions conversion
// ---------------------------------------------------------------------------

/// Convert a ToolDefinition to Anthropic's expected format.
#[cfg(test)]
fn convert_tool_def(tool: &ToolDefinition) -> ApiToolDef {
    ApiToolDef {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.parameters.clone(),
        cache_control: None,
    }
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

/// Parse a complete SSE frame from Anthropic's streaming response.
/// Returns None for non-data lines (comments, empty lines, event-type only lines).
fn parse_sse_event(data: &str) -> Result<Option<SseEvent>> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(None);
    }
    // SSE sends "event: <type>\ndata: <json>" — we only care about data lines.
    // Parse as JSON; unknown event types are caught by #[serde(other)].
    match serde_json::from_str(trimmed) {
        Ok(event) => Ok(Some(event)),
        Err(_e) => {
            // Ignore unparseable events for forward compatibility without writing
            // directly to stderr from library/runtime code.
            Ok(None)
        }
    }
}

/// Process a sequence of SSE events into StreamEvents.
/// This is the core state machine for Anthropic's streaming protocol.
fn process_sse_event(event: SseEvent, state: &mut StreamState) -> Vec<StreamEvent> {
    let mut out = Vec::new();

    match event {
        SseEvent::MessageStart { message } => {
            if let Some(model) = message.model {
                state.model = model.clone();
                out.push(StreamEvent::MessageStart { model });
            }
            if let Some(u) = message.usage {
                state.usage.input_tokens = u.input_tokens;
                state.usage.cache_read_tokens = u.cache_read_input_tokens;
                state.usage.cache_write_tokens = u.cache_creation_input_tokens;
            }
        }
        SseEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            // Ensure blocks vec is large enough
            while state.blocks.len() <= index {
                state.blocks.push(BlockState::Text);
            }
            match content_block {
                SseContentBlock::Text { .. } => {
                    state.blocks[index] = BlockState::Text;
                }
                SseContentBlock::ToolUse { id, name, .. } => {
                    state.blocks[index] = BlockState::ToolUse {
                        id,
                        name,
                        json_buf: String::new(),
                    };
                }
                SseContentBlock::Thinking { .. } => {
                    state.blocks[index] = BlockState::Thinking;
                }
            }
        }
        SseEvent::ContentBlockDelta { index, delta } => {
            if index < state.blocks.len() {
                match delta {
                    SseDelta::TextDelta { text } => {
                        out.push(StreamEvent::TextDelta { text });
                    }
                    SseDelta::ThinkingDelta { thinking } => {
                        out.push(StreamEvent::ThinkingDelta { text: thinking });
                    }
                    SseDelta::InputJsonDelta { partial_json } => {
                        if let BlockState::ToolUse {
                            ref mut json_buf, ..
                        } = state.blocks[index]
                        {
                            json_buf.push_str(&partial_json);
                        }
                    }
                    // Signature and unknown deltas are safely ignored
                    SseDelta::SignatureDelta { .. } | SseDelta::Unknown => {}
                }
            }
        }
        SseEvent::ContentBlockStop { index } => {
            if index < state.blocks.len() {
                match &state.blocks[index] {
                    BlockState::ToolUse { id, name, json_buf } => {
                        let arguments: serde_json::Value = serde_json::from_str(json_buf)
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        let tc = StreamEvent::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        };
                        state.content.push(ContentBlock::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments,
                        });
                        out.push(tc);
                    }
                    BlockState::Text | BlockState::Thinking => {
                        // Text/thinking deltas were already emitted incrementally
                    }
                }
            }
        }
        SseEvent::MessageDelta { delta, usage } => {
            if let Some(reason) = delta.stop_reason {
                state.stop_reason = match reason.as_str() {
                    "end_turn" => StopReason::EndTurn,
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    other => StopReason::Error(other.to_string()),
                };
            }
            if let Some(u) = usage {
                state.usage.output_tokens = u.output_tokens;
            }
        }
        SseEvent::MessageStop => {
            state.finished = true;
            let message = AssistantMessage {
                content: std::mem::take(&mut state.content),
                usage: Some(state.usage.clone()),
                stop_reason: state.stop_reason.clone(),
                timestamp: crate::now(),
            };
            out.push(StreamEvent::MessageEnd { message });
        }
        SseEvent::Ping | SseEvent::Unknown => {}
        SseEvent::Error { error } => {
            out.push(StreamEvent::Error {
                error: error.message,
            });
        }
    }

    out
}

/// Parse raw SSE text from the Anthropic API into StreamEvents.
///
/// The SSE protocol sends lines like:
/// ```text
/// event: message_start
/// data: {"type": "message_start", ...}
///
/// event: content_block_delta
/// data: {"type": "content_block_delta", ...}
/// ```
///
/// We extract "data:" lines and parse them as JSON.
#[cfg(test)]
fn parse_sse_stream(raw: &str, state: &mut StreamState) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            match parse_sse_event(data) {
                Ok(Some(sse)) => {
                    for ev in process_sse_event(sse, state) {
                        events.push(Ok(ev));
                    }
                }
                Ok(None) => {}
                Err(e) => events.push(Err(e)),
            }
        }
    }

    events
}

// ---------------------------------------------------------------------------
// Streaming implementation using channels
// ---------------------------------------------------------------------------

/// Create a streaming response from the Anthropic API.
/// Returns a Stream of StreamEvents.
/// Maximum number of retries for transient errors.
const MAX_RETRIES: u32 = 8;

/// Maximum consecutive 529 (overloaded) errors before giving up.
const MAX_CONSECUTIVE_529: u32 = 3;

/// Floor for max_tokens when recovering from context overflow.
pub const FLOOR_OUTPUT_TOKENS: u32 = 3_000;

/// Default max_tokens cap (matches Claude Code's capped default).
pub const DEFAULT_MAX_TOKENS: u32 = 8_192;

/// Escalated max_tokens for retry after truncation.
pub const ESCALATED_MAX_TOKENS: u32 = 64_000;

/// Check if an HTTP status code is retryable.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 429 | 500 | 502 | 503 | 529)
}

/// Compute backoff delay, honoring retry-after header if present.
fn retry_delay(attempt: u32) -> std::time::Duration {
    let base_ms = 1000u64 * 2u64.pow(attempt.min(5)); // cap at 32s
    let jitter_ms = rand::random::<u64>() % 500;
    std::time::Duration::from_millis(base_ms + jitter_ms)
}

/// Parse retry-after header value (seconds) into a Duration.
fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let val = headers.get("retry-after")?.to_str().ok()?;
    let secs: u64 = val.parse().ok()?;
    Some(std::time::Duration::from_secs(secs))
}

/// Parse context overflow error: "input length and `max_tokens` exceed context limit: X + Y > Z"
pub fn parse_context_overflow(body: &str) -> Option<(u32, u32, u32)> {
    let needle = "input length and `max_tokens` exceed context limit: ";
    let rest = body.find(needle).map(|i| &body[i + needle.len()..])?;
    let parts: Vec<&str> = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() >= 3 {
        let input: u32 = parts[0].parse().ok()?;
        let max: u32 = parts[1].parse().ok()?;
        let limit: u32 = parts[2].parse().ok()?;
        Some((input, max, limit))
    } else {
        None
    }
}

fn stream_response(
    client: reqwest::Client,
    api_key: String,
    request: ApiRequest,
    betas: Vec<&'static str>,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
    let (tx, rx) = futures::channel::mpsc::unbounded();

    tokio::spawn(async move {
        let is_oauth = api_key.starts_with("sk-ant-oat");

        // Retry loop for transient failures (connection drops, 429, 5xx, 529)
        let mut attempt = 0u32;
        let mut consecutive_529 = 0u32;
        let mut had_401 = false;
        let resp = loop {
            let mut req = client
                .post(API_URL)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json");

            let mut request_betas = betas.clone();

            if is_oauth {
                request_betas.insert(0, "oauth-2025-04-20");
                req = req
                    .header("authorization", format!("Bearer {api_key}"))
                    .header("anthropic-dangerous-direct-browser-access", "true");
            } else {
                req = req.header("x-api-key", &api_key);
            }

            req = req.header("anthropic-beta", request_betas.join(","));

            let result = req.json(&request).send().await;

            match result {
                Ok(r) => {
                    let status = r.status();
                    if status.is_success() {
                        break r;
                    }

                    // Track consecutive 529 (overloaded) errors
                    if status.as_u16() == 529 {
                        consecutive_529 += 1;
                        if consecutive_529 >= MAX_CONSECUTIVE_529 {
                            let body = crate::auth::redact_provider_error_body(
                                &r.text().await.unwrap_or_default(),
                            );
                            let _ = tx.unbounded_send(Err(Error::Provider(format!(
                                "API overloaded after {} consecutive 529 errors: {body}",
                                MAX_CONSECUTIVE_529
                            ))));
                            return;
                        }
                    } else {
                        consecutive_529 = 0;
                    }

                    // 401: retry once (token may have expired)
                    if status.as_u16() == 401 {
                        if had_401 {
                            let body = crate::auth::redact_provider_error_body(
                                &r.text().await.unwrap_or_default(),
                            );
                            let _ = tx.unbounded_send(Err(Error::Provider(format!(
                                "HTTP 401 (authentication failed): {body}"
                            ))));
                            return;
                        }
                        had_401 = true;
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        continue;
                    }

                    // Retryable HTTP error
                    if is_retryable_status(status) && attempt < MAX_RETRIES {
                        // Honor retry-after header if present
                        let delay =
                            retry_after_delay(r.headers()).unwrap_or_else(|| retry_delay(attempt));
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    // Non-retryable or exhausted retries
                    let body = crate::auth::redact_provider_error_body(
                        &r.text().await.unwrap_or_default(),
                    );
                    let _ =
                        tx.unbounded_send(Err(Error::Provider(format!("HTTP {status}: {body}"))));
                    return;
                }
                Err(e) => {
                    // Connection/timeout errors are retryable
                    let is_transient = e.is_connect() || e.is_timeout() || e.is_request();
                    if is_transient && attempt < MAX_RETRIES {
                        let delay = retry_delay(attempt);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    let _ = tx.unbounded_send(Err(Error::Http(e)));
                    return;
                }
            }
        };

        let mut state = StreamState::new();
        let mut buf = String::new();
        let mut byte_stream = resp.bytes_stream();

        use futures::StreamExt;
        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));

                    // Process complete lines
                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf = buf[pos + 1..].to_string();

                        let trimmed = line.trim();
                        if let Some(data) = trimmed.strip_prefix("data: ") {
                            match parse_sse_event(data) {
                                Ok(Some(sse)) => {
                                    for ev in process_sse_event(sse, &mut state) {
                                        if tx.unbounded_send(Ok(ev)).is_err() {
                                            return;
                                        }
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    if tx.unbounded_send(Err(e)).is_err() {
                                        return;
                                    }
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
        if let Some(data) = trimmed.strip_prefix("data: ") {
            match parse_sse_event(data) {
                Ok(Some(sse)) => {
                    for ev in process_sse_event(sse, &mut state) {
                        if tx.unbounded_send(Ok(ev)).is_err() {
                            return;
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

        if !state.finished {
            let _ = tx.unbounded_send(Err(Error::Stream(
                "Anthropic stream ended before message_stop".into(),
            )));
        }
    });

    Box::pin(rx)
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn stream(
        &self,
        model: &Model,
        context: Context,
        options: RequestOptions,
        api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
        // OAuth tokens are scoped to Claude Code's identity. Anthropic rejects
        // requests with custom system prompts or tool definitions that don't match
        // the expected Claude Code format. When using OAuth:
        // 1. Always use the required system prompt
        // 2. Prepend any custom instructions to the first user message
        let mut options = options;
        let mut context = context;
        let oauth_system = "You are Claude Code, Anthropic's official CLI for Claude.".to_string();
        if api_key.starts_with("sk-ant-oat") {
            if !options.system_prompt.is_empty() && options.system_prompt != oauth_system {
                // Move custom system prompt into user message context
                let prefix = format!(
                    "<instructions>\n{}\n</instructions>\n\n",
                    options.system_prompt
                );
                if let Some(crate::message::Message::User(user_msg)) = context.messages.first_mut()
                {
                    let original = user_msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            crate::message::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    user_msg.content = vec![crate::message::ContentBlock::Text {
                        text: format!("{prefix}{original}"),
                    }];
                }
            }
            options.system_prompt = oauth_system;
        }
        let effort = options.effort;
        let request = build_request(model, context, options);
        let client = self.client.clone();
        let api_key = api_key.to_string();
        let betas = beta_headers(&model.meta, effort);
        stream_response(client, api_key, request, betas)
    }

    async fn resolve_auth(&self, auth: &AuthStore) -> Result<ApiKey> {
        auth.resolve("anthropic")
    }

    fn id(&self) -> &str {
        "anthropic"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
