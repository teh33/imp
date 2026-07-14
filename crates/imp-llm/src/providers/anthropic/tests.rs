use super::*;
use crate::message::{ToolResultMessage, UserMessage};
use crate::provider::CacheOptions;

// -- Request serialization tests --

#[test]
fn serialize_text_user_message() {
    let msg = Message::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "Hello".into(),
        }],
        timestamp: 0,
    });
    let api = convert_message(&msg);
    assert_eq!(api.role, "user");
    let json = serde_json::to_value(&api.content).unwrap();
    assert_eq!(json[0]["type"], "text");
    assert_eq!(json[0]["text"], "Hello");
}

#[test]
fn serialize_image_content_block() {
    let block = ContentBlock::Image {
        media_type: "image/png".into(),
        data: "iVBOR...".into(),
    };
    let api = convert_content_block(&block);
    let json = serde_json::to_value(&api).unwrap();
    assert_eq!(json["type"], "image");
    assert_eq!(json["source"]["type"], "base64");
    assert_eq!(json["source"]["media_type"], "image/png");
    assert_eq!(json["source"]["data"], "iVBOR...");
}

#[test]
fn serialize_tool_call_block() {
    let block = ContentBlock::ToolCall {
        id: "call_1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({"command": "ls"}),
    };
    let api = convert_content_block(&block);
    let json = serde_json::to_value(&api).unwrap();
    assert_eq!(json["type"], "tool_use");
    assert_eq!(json["id"], "call_1");
    assert_eq!(json["name"], "bash");
    assert_eq!(json["input"]["command"], "ls");
}

#[test]
fn serialize_tool_result_message() {
    let msg = Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "file.txt".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    });
    let api = convert_message(&msg);
    assert_eq!(api.role, "user");
    let json = serde_json::to_value(&api.content).unwrap();
    assert_eq!(json[0]["type"], "tool_result");
    assert_eq!(json[0]["tool_use_id"], "call_1");
}

#[test]
fn serialize_tool_result_with_error() {
    let msg = Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_2".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "permission denied".into(),
        }],
        is_error: true,
        details: serde_json::Value::Null,
        timestamp: 0,
    });
    let api = convert_message(&msg);
    let json = serde_json::to_value(&api.content).unwrap();
    assert_eq!(json[0]["is_error"], true);
}

#[test]
fn serialize_thinking_block() {
    let block = ContentBlock::Thinking {
        text: "Let me think...".into(),
    };
    let api = convert_content_block(&block);
    let json = serde_json::to_value(&api).unwrap();
    assert_eq!(json["type"], "thinking");
    assert_eq!(json["thinking"], "Let me think...");
}

#[test]
fn serialize_assistant_message() {
    let msg = Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "Here:".into(),
            },
            ContentBlock::ToolCall {
                id: "tc_1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
            },
        ],
        usage: None,
        stop_reason: StopReason::ToolUse,
        timestamp: 0,
    });
    let api = convert_message(&msg);
    assert_eq!(api.role, "assistant");
    assert_eq!(api.content.len(), 2);
    let json = serde_json::to_value(&api.content).unwrap();
    assert_eq!(json[0]["type"], "text");
    assert_eq!(json[1]["type"], "tool_use");
}

// -- Cache control tests --

#[test]
fn cache_system_prompt() {
    let cache = CacheOptions {
        cache_system_prompt: true,
        cache_tools: false,
        cache_recent_turns: 0,
        ..Default::default()
    };
    let blocks = build_system_blocks("You are helpful.", &cache);
    let json = serde_json::to_value(&blocks[0]).unwrap();
    assert_eq!(json["cache_control"]["type"], "ephemeral");
}

#[test]
fn no_cache_system_prompt() {
    let cache = CacheOptions::default();
    let blocks = build_system_blocks("You are helpful.", &cache);
    let json = serde_json::to_value(&blocks[0]).unwrap();
    assert!(json.get("cache_control").is_none());
}

#[test]
fn cache_on_last_tool_def() {
    let tools = vec![
        ToolDefinition {
            name: "read".into(),
            description: "Read file".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
        ToolDefinition {
            name: "write".into(),
            description: "Write file".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
    ];
    let cache = CacheOptions {
        cache_system_prompt: false,
        cache_tools: true,
        cache_recent_turns: 0,
        ..Default::default()
    };
    let api_tools = build_tool_defs(&tools, &cache);
    assert!(api_tools[0].cache_control.is_none());
    assert!(api_tools[1].cache_control.is_some());
}

#[test]
fn cache_recent_user_turns() {
    let messages = vec![
        Message::user("first"),
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "reply".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 0,
        }),
        Message::user("second"),
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "reply2".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 0,
        }),
        Message::user("third"),
    ];
    let cache = CacheOptions {
        cache_system_prompt: false,
        cache_tools: false,
        cache_recent_turns: 2,
        ..Default::default()
    };
    let api_msgs = build_messages(&messages, &cache);

    // Last 2 user messages (indices 2 and 4) should have cache_control
    // First user (index 0) should not
    let json0 = serde_json::to_value(&api_msgs[0].content).unwrap();
    assert!(json0[0].get("cache_control").is_none());

    let json2 = serde_json::to_value(&api_msgs[2].content).unwrap();
    assert_eq!(json2[0]["cache_control"]["type"], "ephemeral");

    let json4 = serde_json::to_value(&api_msgs[4].content).unwrap();
    assert_eq!(json4[0]["cache_control"]["type"], "ephemeral");
}

// -- Thinking budget tests --

#[test]
fn thinking_budget_off() {
    assert_eq!(thinking_budget(ThinkingLevel::Off), None);
}

#[test]
fn thinking_budget_minimal() {
    assert_eq!(thinking_budget(ThinkingLevel::Minimal), Some(1024));
}

#[test]
fn thinking_budget_low() {
    assert_eq!(thinking_budget(ThinkingLevel::Low), Some(4096));
}

#[test]
fn thinking_budget_medium() {
    assert_eq!(thinking_budget(ThinkingLevel::Medium), Some(10_000));
}

#[test]
fn thinking_budget_high() {
    assert_eq!(thinking_budget(ThinkingLevel::High), Some(32_000));
}

#[test]
fn thinking_budget_xhigh() {
    assert_eq!(thinking_budget(ThinkingLevel::XHigh), Some(100_000));
}

#[test]
fn test_beta_headers_large_context() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-6".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };

    let betas = beta_headers(&model_meta, None);
    assert!(betas.contains(&"interleaved-thinking-2025-05-14"));
    assert!(betas.contains(&"prompt-caching-scope-2026-01-05"));
    assert!(betas.contains(&"context-1m-2025-08-07"));
}

#[test]
fn test_beta_headers_standard_context() {
    let model_meta = ModelMeta {
        id: "claude-haiku-3-5-20241022".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 8_192,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };

    let betas = beta_headers(&model_meta, None);
    assert!(betas.contains(&"interleaved-thinking-2025-05-14"));
    assert!(betas.contains(&"prompt-caching-scope-2026-01-05"));
    assert!(!betas.contains(&"context-1m-2025-08-07"));
}

#[test]
fn test_beta_headers_always_includes_interleaved() {
    let standard = ModelMeta {
        id: "claude-haiku-3-5-20241022".into(),
        provider: "anthropic".into(),
        name: "standard".into(),
        context_window: 200_000,
        max_output_tokens: 8_192,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let large = ModelMeta {
        id: "claude-opus-4-6".into(),
        provider: "anthropic".into(),
        name: "large".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };

    assert!(beta_headers(&standard, None).contains(&"interleaved-thinking-2025-05-14"));
    assert!(beta_headers(&large, None).contains(&"interleaved-thinking-2025-05-14"));
}

#[test]
fn default_max_tokens_caps_large_models() {
    let model_meta = ModelMeta {
        id: "claude-opus-4-6".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert_eq!(req.max_tokens, 8_192);
    assert!(req.thinking.is_none());
}

#[test]
fn thinking_forces_max_tokens_above_budget() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 4096,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let context = Context::default();
    let options = RequestOptions {
        thinking_level: ThinkingLevel::High,
        max_tokens: None,
        ..Default::default()
    };
    let req = build_request(&model, context, options);
    // Budget is 32000, max_output is 4096. Should be bumped to 33024.
    assert!(req.max_tokens > 32_000);
    assert!(req.thinking.is_some());
    let t = serde_json::to_value(req.thinking.unwrap()).unwrap();
    assert_eq!(
        t,
        serde_json::json!({"type": "enabled", "budget_tokens": 32_000})
    );
}

#[test]
fn thinking_off_allows_temperature() {
    let model_meta = ModelMeta {
        id: "claude-haiku-3-5-20241022".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 8192,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Off,
        temperature: Some(0.5),
        ..Default::default()
    };
    let req = build_request(&model, Context::default(), options);
    assert_eq!(req.temperature, Some(0.5));
    assert!(req.thinking.is_none());
}

#[test]
fn test_adaptive_thinking_sonnet_46() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-6".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Medium,
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    let thinking_json = serde_json::to_value(req.thinking.unwrap()).unwrap();
    assert_eq!(thinking_json, serde_json::json!({"type": "adaptive"}));
}

#[test]
fn test_budget_thinking_sonnet_40() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Medium,
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    let thinking_json = serde_json::to_value(req.thinking.unwrap()).unwrap();
    assert_eq!(
        thinking_json,
        serde_json::json!({"type": "enabled", "budget_tokens": 10_000})
    );
}

#[test]
fn test_adaptive_still_caps_low_levels() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-6".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Minimal,
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    let thinking_json = serde_json::to_value(req.thinking.unwrap()).unwrap();
    assert_eq!(
        thinking_json,
        serde_json::json!({"type": "enabled", "budget_tokens": 1024})
    );
}

#[test]
fn thinking_enabled_strips_temperature() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Medium,
        temperature: Some(0.7),
        ..Default::default()
    };
    let req = build_request(&model, Context::default(), options);
    assert!(req.temperature.is_none());
    assert!(req.thinking.is_some());
}

#[test]
fn test_adaptive_max_tokens_not_capped() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4.6".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 1_000_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        thinking_level: ThinkingLevel::Medium,
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    assert_eq!(req.max_tokens, 128_000);
    let thinking_json = serde_json::to_value(req.thinking.unwrap()).unwrap();
    assert_eq!(thinking_json, serde_json::json!({"type": "adaptive"}));
}

// -- SSE parsing tests --

#[test]
fn parse_message_start_event() {
    let data = r#"{"type":"message_start","message":{"model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":50,"cache_creation_input_tokens":10}}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], StreamEvent::MessageStart { model } if model == "claude-sonnet-4-20250514")
    );
    assert_eq!(state.usage.input_tokens, 160);
    assert_eq!(state.usage.cache_read_tokens, 50);
    assert_eq!(state.usage.cache_write_tokens, 10);
}

#[test]
fn parse_text_delta_event() {
    let data =
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    state.blocks.push(BlockState::Text);
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "Hello"));
}

#[test]
fn parse_thinking_delta_event() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"reasoning..."}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    state.blocks.push(BlockState::Thinking);
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::ThinkingDelta { text } if text == "reasoning..."));
}

#[test]
fn parse_tool_use_accumulates_json() {
    let mut state = StreamState::new();

    // content_block_start for tool_use
    let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}"#;
    let event = parse_sse_event(start).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    // Two delta chunks
    let d1 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"com"}}"#;
    let event = parse_sse_event(d1).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    let d2 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"mand\":\"ls\"}"}}"#;
    let event = parse_sse_event(d2).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    // content_block_stop emits the tool call
    let stop = r#"{"type":"content_block_stop","index":0}"#;
    let event = parse_sse_event(stop).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    if let StreamEvent::ToolCall {
        id,
        name,
        arguments,
    } = &events[0]
    {
        assert_eq!(id, "toolu_1");
        assert_eq!(name, "bash");
        assert_eq!(arguments["command"], "ls");
    } else {
        panic!("expected ToolCall event");
    }
}

#[test]
fn parse_message_delta_and_stop() {
    let mut state = StreamState::new();
    state.model = "claude-sonnet-4-20250514".into();

    let delta = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
    let event = parse_sse_event(delta).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());
    assert_eq!(state.stop_reason, StopReason::EndTurn);
    assert_eq!(state.usage.output_tokens, 42);

    let stop = r#"{"type":"message_stop"}"#;
    let event = parse_sse_event(stop).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], StreamEvent::MessageEnd { message } if message.stop_reason == StopReason::EndTurn)
    );
}

#[test]
fn parse_tool_use_stop_reason() {
    let mut state = StreamState::new();
    let delta = r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":10}}"#;
    let event = parse_sse_event(delta).unwrap().unwrap();
    process_sse_event(event, &mut state);
    assert_eq!(state.stop_reason, StopReason::ToolUse);
}

#[test]
fn parse_max_tokens_stop_reason() {
    let mut state = StreamState::new();
    let delta = r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":10}}"#;
    let event = parse_sse_event(delta).unwrap().unwrap();
    process_sse_event(event, &mut state);
    assert_eq!(state.stop_reason, StopReason::MaxTokens);
}

#[test]
fn parse_error_event() {
    let data = r#"{"type":"error","error":{"message":"Overloaded"}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::Error { error } if error == "Overloaded"));
}

#[test]
fn parse_ping_event() {
    let data = r#"{"type":"ping"}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());
}

#[test]
fn parse_full_sse_stream() {
    let raw = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":10,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi!\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\
";
    let mut state = StreamState::new();
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    // MessageStart, TextDelta("Hi!"), MessageEnd
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(&events[1], StreamEvent::TextDelta { text } if text == "Hi!"));
    assert!(matches!(&events[2], StreamEvent::MessageEnd { .. }));
}

#[test]
fn full_request_round_trip_json() {
    // Build a realistic request and verify it serializes to expected Anthropic format
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };

    let context = Context {
        messages: vec![
            Message::user("What files are in this directory?"),
            Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "tc_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                }],
                usage: None,
                stop_reason: StopReason::ToolUse,
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "tc_1".into(),
                tool_name: "bash".into(),
                content: vec![ContentBlock::Text {
                    text: "README.md\nsrc/".into(),
                }],
                is_error: false,
                details: serde_json::Value::Null,
                timestamp: 0,
            }),
        ],
        ..Default::default()
    };

    let options = RequestOptions {
        system_prompt: "You are a helpful assistant.".into(),
        tools: vec![ToolDefinition {
            name: "bash".into(),
            description: "Run a bash command".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
        }],
        cache_options: CacheOptions {
            cache_system_prompt: true,
            cache_tools: true,
            cache_recent_turns: 1,
            ..Default::default()
        },
        ..Default::default()
    };

    let req = build_request(&model, context, options);
    let json = serde_json::to_value(&req).unwrap();

    // Verify structure
    assert_eq!(json["model"], "claude-sonnet-4-20250514");
    assert_eq!(json["stream"], true);
    assert!(json["max_tokens"].as_u64().unwrap() > 0);

    // System has cache_control
    assert_eq!(json["system"][0]["cache_control"]["type"], "ephemeral");

    // Tools has cache_control on last
    assert_eq!(json["tools"][0]["cache_control"]["type"], "ephemeral");

    // Messages structure
    assert_eq!(json["messages"].as_array().unwrap().len(), 3);
    assert_eq!(json["messages"][0]["role"], "user");
    assert_eq!(json["messages"][1]["role"], "assistant");
    assert_eq!(json["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(json["messages"][2]["role"], "user");
    assert_eq!(json["messages"][2]["content"][0]["type"], "tool_result");
}

// -- Tool definition conversion test --

#[test]
fn convert_tool_definition() {
    let tool = ToolDefinition {
        name: "read_file".into(),
        description: "Read a file from disk".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" }
            },
            "required": ["path"]
        }),
    };
    let api = convert_tool_def(&tool);
    let json = serde_json::to_value(&api).unwrap();
    assert_eq!(json["name"], "read_file");
    assert_eq!(json["description"], "Read a file from disk");
    assert_eq!(json["input_schema"]["type"], "object");
    assert_eq!(json["input_schema"]["properties"]["path"]["type"], "string");
}

// -- Edge case: SSE parsing --

#[test]
fn parse_sse_event_empty_string_returns_none() {
    let result = parse_sse_event("").unwrap();
    assert!(result.is_none());
}

#[test]
fn parse_sse_event_whitespace_only_returns_none() {
    let result = parse_sse_event("   \n  ").unwrap();
    assert!(result.is_none());
}

#[test]
fn parse_sse_event_malformed_json_returns_none() {
    // Malformed JSON is treated as an unparseable event and skipped
    // (forward compatibility — don't crash on unknown SSE event formats).
    let result = parse_sse_event("{not valid json}");
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn sse_stream_skips_non_data_lines() {
    // Lines without "data: " prefix should be ignored
    let raw = "\
event: message_start\n\
: this is a comment\n\
data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}\n\
\n\
some random line\n\
data: {\"type\":\"message_stop\"}\n";
    let mut state = StreamState::new();
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events.into_iter().filter_map(|e| e.ok()).collect();
    // Should get MessageStart and MessageEnd only
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(&events[1], StreamEvent::MessageEnd { .. }));
}

#[test]
fn tool_call_with_empty_json_arguments() {
    let mut state = StreamState::new();

    let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_empty","name":"noop","input":{}}}"#;
    let event = parse_sse_event(start).unwrap().unwrap();
    process_sse_event(event, &mut state);

    // Empty JSON object as the accumulated buffer
    let d1 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#;
    let event = parse_sse_event(d1).unwrap().unwrap();
    process_sse_event(event, &mut state);

    let stop = r#"{"type":"content_block_stop","index":0}"#;
    let event = parse_sse_event(stop).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert_eq!(events.len(), 1);
    if let StreamEvent::ToolCall { arguments, .. } = &events[0] {
        assert!(arguments.is_object());
        assert!(arguments.as_object().unwrap().is_empty());
    } else {
        panic!("expected ToolCall");
    }
}

#[test]
fn message_delta_missing_usage_defaults_to_zero() {
    let mut state = StreamState::new();
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    process_sse_event(event, &mut state);
    // output_tokens should remain 0 since no usage was provided
    assert_eq!(state.usage.output_tokens, 0);
    assert_eq!(state.stop_reason, StopReason::EndTurn);
}

#[test]
fn unknown_stop_reason_maps_to_error() {
    let mut state = StreamState::new();
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"content_filter"},"usage":{"output_tokens":0}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    process_sse_event(event, &mut state);
    assert!(matches!(state.stop_reason, StopReason::Error(ref s) if s == "content_filter"));
}

#[test]
fn content_block_delta_out_of_range_ignored() {
    let mut state = StreamState::new();
    // index 5, but no blocks exist — should not panic
    let data =
        r#"{"type":"content_block_delta","index":5,"delta":{"type":"text_delta","text":"oops"}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());
}

#[test]
fn content_block_stop_out_of_range_ignored() {
    let mut state = StreamState::new();
    // index 3, but no blocks — should not panic
    let data = r#"{"type":"content_block_stop","index":3}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());
}

// -- Edge case: request building --

#[test]
fn build_request_empty_system_prompt_produces_no_system_blocks() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        system_prompt: "".into(),
        ..Default::default()
    };
    let req = build_request(&model, Context::default(), options);
    assert!(req.system.is_empty());
}

#[test]
fn build_request_empty_tools_produces_no_tools() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        tools: vec![],
        ..Default::default()
    };
    let req = build_request(&model, Context::default(), options);
    assert!(req.tools.is_empty());
    // Verify it serializes without a "tools" key
    let json = serde_json::to_value(&req).unwrap();
    assert!(json.get("tools").is_none());
}

#[test]
fn cache_zero_recent_turns_adds_no_breakpoints() {
    let messages = vec![Message::user("first"), Message::user("second")];
    let cache = CacheOptions {
        cache_system_prompt: false,
        cache_tools: false,
        cache_recent_turns: 0,
        ..Default::default()
    };
    let api_msgs = build_messages(&messages, &cache);
    for msg in &api_msgs {
        for block in &msg.content {
            let json = serde_json::to_value(block).unwrap();
            assert!(json.get("cache_control").is_none());
        }
    }
}

// -- Effort level tests (41.3) --

#[test]
fn test_effort_level_serialization() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    for (level, expected) in [
        (EffortLevel::Low, "low"),
        (EffortLevel::Medium, "medium"),
        (EffortLevel::High, "high"),
    ] {
        let options = RequestOptions {
            effort: Some(level),
            ..Default::default()
        };
        let req = build_request(&model, Context::default(), options);
        let json = serde_json::to_value(&req.output_config).unwrap();
        assert_eq!(json["effort"], expected);
    }
}

#[test]
fn test_effort_none_omits_field() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = AnthropicProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let req = build_request(&model, Context::default(), RequestOptions::default());
    let json = serde_json::to_value(&req).unwrap();
    assert!(json.get("output_config").is_none());
}

#[test]
fn test_effort_adds_beta_header() {
    let model_meta = ModelMeta {
        id: "claude-sonnet-4-20250514".into(),
        provider: "anthropic".into(),
        name: "test".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let betas = beta_headers(&model_meta, Some(EffortLevel::Medium));
    assert!(betas.contains(&"effort-2025-11-24"));

    let betas_none = beta_headers(&model_meta, None);
    assert!(!betas_none.contains(&"effort-2025-11-24"));
}

// -- Retry logic tests (41.4) --

#[test]
fn test_retry_after_header_parsing() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "5".parse().unwrap());
    let delay = retry_after_delay(&headers).unwrap();
    assert_eq!(delay, std::time::Duration::from_secs(5));
}

#[test]
fn test_retry_after_missing() {
    let headers = reqwest::header::HeaderMap::new();
    assert!(retry_after_delay(&headers).is_none());
}

#[test]
fn test_context_overflow_parsing() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"input length and `max_tokens` exceed context limit: 188059 + 20000 > 200000"}}"#;
    let result = parse_context_overflow(body).unwrap();
    assert_eq!(result, (188059, 20000, 200000));
}

#[test]
fn test_context_overflow_no_match() {
    assert!(parse_context_overflow("some other error").is_none());
}

// -- Tool sorting tests (41.7) --

#[test]
fn test_tool_defs_sorted_alphabetically() {
    let tools = vec![
        ToolDefinition {
            name: "write".into(),
            description: "Write".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
        ToolDefinition {
            name: "bash".into(),
            description: "Bash".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
        ToolDefinition {
            name: "read".into(),
            description: "Read".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
    ];
    let cache = CacheOptions::default();
    let api_tools = build_tool_defs(&tools, &cache);
    assert_eq!(api_tools[0].name, "bash");
    assert_eq!(api_tools[1].name, "read");
    assert_eq!(api_tools[2].name, "write");
}

#[test]
fn test_tool_cache_breakpoint_on_last_sorted() {
    let tools = vec![
        ToolDefinition {
            name: "write".into(),
            description: "Write".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
        ToolDefinition {
            name: "bash".into(),
            description: "Bash".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
    ];
    let cache = CacheOptions {
        cache_tools: true,
        ..Default::default()
    };
    let api_tools = build_tool_defs(&tools, &cache);
    // After sorting: bash, write. Cache on write (last).
    assert_eq!(api_tools[0].name, "bash");
    assert!(api_tools[0].cache_control.is_none());
    assert_eq!(api_tools[1].name, "write");
    assert!(api_tools[1].cache_control.is_some());
}

// -- Cache TTL/scope tests (41.9) --

#[test]
fn test_cache_ttl_default() {
    let cache = CacheOptions::default();
    let cc = make_cache_control(&cache).unwrap();
    let json = serde_json::to_value(&cc).unwrap();
    assert_eq!(json["type"], "ephemeral");
    assert!(json.get("ttl").is_none());
    assert!(json.get("scope").is_none());
}

#[test]
fn test_cache_ttl_extended() {
    let cache = CacheOptions {
        extended_ttl: true,
        ..Default::default()
    };
    let cc = make_cache_control(&cache).unwrap();
    let json = serde_json::to_value(&cc).unwrap();
    assert_eq!(json["type"], "ephemeral");
    assert_eq!(json["ttl"], "1h");
}

#[test]
fn test_cache_ttl_global_scope() {
    let cache = CacheOptions {
        global_scope: true,
        ..Default::default()
    };
    let cc = make_cache_control(&cache).unwrap();
    let json = serde_json::to_value(&cc).unwrap();
    assert_eq!(json["scope"], "global");
}

#[test]
fn test_cache_ttl_both() {
    let cache = CacheOptions {
        extended_ttl: true,
        global_scope: true,
        ..Default::default()
    };
    let cc = make_cache_control(&cache).unwrap();
    let json = serde_json::to_value(&cc).unwrap();
    assert_eq!(json["ttl"], "1h");
    assert_eq!(json["scope"], "global");
}

// -- max_tokens escalation constants (41.5) --

#[test]
fn test_max_tokens_escalation_constants() {
    assert_eq!(DEFAULT_MAX_TOKENS, 8_192);
    assert_eq!(ESCALATED_MAX_TOKENS, 64_000);
    const _: () = assert!(ESCALATED_MAX_TOKENS > DEFAULT_MAX_TOKENS);
}

// -- Non-streaming fallback tests (41.6) --

#[test]
fn test_non_streaming_response_parsing() {
    let json = r#"{
        "model": "claude-sonnet-4-20250514",
        "content": [
            {"type": "text", "text": "Hello world"},
            {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 10, "cache_creation_input_tokens": 5}
    }"#;
    let resp: ApiResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.model, "claude-sonnet-4-20250514");
    assert_eq!(resp.content.len(), 2);
    assert_eq!(resp.stop_reason, Some("tool_use".to_string()));
}

#[test]
fn test_nonstreaming_response_to_events_conversion() {
    let resp = ApiResponse {
        model: "claude-sonnet-4-20250514".into(),
        content: vec![
            ApiResponseBlock::Text { text: "Hi".into() },
            ApiResponseBlock::ToolUse {
                id: "t1".into(),
                name: "read".into(),
                input: serde_json::json!({"path": "foo.rs"}),
            },
        ],
        stop_reason: Some("tool_use".into()),
        usage: SseUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        },
    };
    let events = non_streaming_response_to_events(resp);
    // MessageStart, TextDelta, ToolCall, MessageEnd
    assert_eq!(events.len(), 4);
    assert!(
        matches!(&events[0], StreamEvent::MessageStart { model } if model == "claude-sonnet-4-20250514")
    );
    assert!(matches!(&events[1], StreamEvent::TextDelta { text } if text == "Hi"));
    assert!(matches!(&events[2], StreamEvent::ToolCall { name, .. } if name == "read"));
    assert!(
        matches!(&events[3], StreamEvent::MessageEnd { message } if message.stop_reason == StopReason::ToolUse)
    );
}
