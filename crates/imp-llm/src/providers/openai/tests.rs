use super::*;
use crate::message::{ToolResultMessage, UserMessage};
use crate::model::{Capabilities, ModelPricing};

#[test]
fn openai_tool_defs_are_sorted_for_prompt_cache_stability() {
    let write = ToolDefinition {
        name: "write".into(),
        description: "Write".into(),
        parameters: serde_json::json!({ "type": "object" }),
    };
    let bash = ToolDefinition {
        name: "bash".into(),
        description: "Bash".into(),
        parameters: serde_json::json!({ "type": "object" }),
    };
    let read = ToolDefinition {
        name: "read".into(),
        description: "Read".into(),
        parameters: serde_json::json!({ "type": "object" }),
    };

    let names = build_tool_defs(&[write.clone(), bash.clone(), read.clone()])
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["bash", "read", "write"]);

    let first_request = ApiRequest {
        model: "gpt-test".into(),
        input: vec![serde_json::json!({ "role": "user", "content": "hello" })],
        stream: true,
        instructions: Some("system".into()),
        tools: build_tool_defs(&[write.clone(), bash.clone(), read.clone()]),
        temperature: None,
        max_output_tokens: None,
        reasoning: None,
    };
    let second_request = ApiRequest {
        model: "gpt-test".into(),
        input: vec![serde_json::json!({ "role": "user", "content": "hello" })],
        stream: true,
        instructions: Some("system".into()),
        tools: build_tool_defs(&[read, write, bash]),
        temperature: None,
        max_output_tokens: None,
        reasoning: None,
    };

    assert_eq!(
        serde_json::to_value(first_request).unwrap(),
        serde_json::to_value(second_request).unwrap()
    );
}

#[test]
fn openai_serialize_text_user_message() {
    let messages = vec![Message::user("Hello, world!")];
    let items = convert_messages(&messages);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["content"], "Hello, world!");
}

#[test]
fn openai_serialize_user_message_with_image() {
    let messages = vec![Message::User(UserMessage {
        content: vec![
            ContentBlock::Text {
                text: "What's in this image?".into(),
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "iVBOR".into(),
            },
        ],
        timestamp: 0,
    })];
    let items = convert_messages(&messages);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["role"], "user");
    let content = items[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "What's in this image?");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,iVBOR");
}

#[test]
fn openai_serialize_assistant_text_message() {
    let messages = vec![Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "Hello!".into(),
        }],
        usage: None,
        stop_reason: StopReason::EndTurn,
        timestamp: 0,
    })];
    let items = convert_messages(&messages);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["role"], "assistant");
    let content = items[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "output_text");
    assert_eq!(content[0]["text"], "Hello!");
}

#[test]
fn openai_serialize_assistant_with_tool_call() {
    let messages = vec![Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "Let me check.".into(),
            },
            ContentBlock::ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls"}),
            },
        ],
        usage: None,
        stop_reason: StopReason::ToolUse,
        timestamp: 0,
    })];
    let items = convert_messages(&messages);
    // Text → message item, tool call → function_call item
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["role"], "assistant");
    assert_eq!(items[1]["type"], "function_call");
    assert_eq!(items[1]["call_id"], "call_1");
    assert_eq!(items[1]["name"], "bash");
    assert_eq!(items[1]["arguments"], "{\"command\":\"ls\"}");
}

#[test]
fn openai_serialize_tool_result() {
    let messages = vec![Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "README.md\nsrc/".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    })];
    let items = convert_messages(&messages);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "function_call_output");
    assert_eq!(items[0]["call_id"], "call_1");
    assert_eq!(items[0]["output"], "README.md\nsrc/");
}

#[test]
fn openai_image_workaround_tool_result_with_image() {
    let messages = vec![Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_screenshot".into(),
        tool_name: "screenshot".into(),
        content: vec![
            ContentBlock::Text {
                text: "Screenshot taken".into(),
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "iVBOR_screenshot".into(),
            },
        ],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    })];
    let items = convert_messages(&messages);

    // Should produce 2 items: function_call_output + user message with image
    assert_eq!(items.len(), 2);

    // First: function_call_output with placeholder
    assert_eq!(items[0]["type"], "function_call_output");
}

// -- SSE parsing tests --

#[test]
fn openai_parse_text_delta() {
    let data = r#"{"type":"response.content_part.delta","delta":"Hello world"}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::TextDelta { text } if text == "Hello world"));
    assert!(matches!(
        state.content.as_slice(),
        [ContentBlock::Text { text }] if text == "Hello world"
    ));
}

#[test]
fn openai_parse_output_text_delta_builds_message_content() {
    let mut state = StreamState::new();

    for data in [
        r#"{"type":"response.output_text.delta","delta":"Hello"}"#,
        r#"{"type":"response.output_text.delta","delta":" world"}"#,
    ] {
        let event = parse_sse_event(data).unwrap().unwrap();
        let events = process_sse_event(event, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::TextDelta { .. }));
    }

    let completed = r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":2}}}"#;
    let event = parse_sse_event(completed).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert_eq!(events.len(), 1);
    if let StreamEvent::MessageEnd { message } = &events[0] {
        assert!(matches!(
            message.content.as_slice(),
            [ContentBlock::Text { text }] if text == "Hello world"
        ));
        let usage = message.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 2);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn openai_parse_reasoning_text_delta() {
    let data = r#"{"type":"response.reasoning_text.delta","delta":"Planning"}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);

    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::ThinkingDelta { text } if text == "Planning"));
    assert!(matches!(
        state.content.as_slice(),
        [ContentBlock::Thinking { text }] if text == "Planning"
    ));
}

#[test]
fn openai_parse_function_call_accumulation() {
    let mut state = StreamState::new();

    // output_item.added for a function_call
    let added = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","name":"bash","call_id":"call_42"}}"#;
    let event = parse_sse_event(added).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    // argument deltas
    let d1 =
        r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"com"}"#;
    let event = parse_sse_event(d1).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    let d2 = r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"mand\":\"ls\"}"}"#;
    let event = parse_sse_event(d2).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());

    // Verify the args buffer accumulated correctly
    if let OutputItemState::FunctionCall { args_buf, .. } = &state.items[0] {
        assert_eq!(args_buf, r#"{"command":"ls"}"#);
    } else {
        panic!("expected FunctionCall state");
    }
}

#[test]
fn openai_parse_response_completed() {
    let mut state = StreamState::new();
    state.model = "gpt-4o".into();

    let data = r#"{"type":"response.completed","response":{"model":"gpt-4o","status":"completed","usage":{"input_tokens":50,"output_tokens":25,"input_tokens_details":{"cached_tokens":10}}}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert_eq!(events.len(), 1);
    if let StreamEvent::MessageEnd { message } = &events[0] {
        assert_eq!(message.stop_reason, StopReason::EndTurn);
        let usage = message.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.cache_read_tokens, 10);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn openai_response_incomplete_maps_to_max_tokens() {
    let mut state = StreamState::new();
    let data = r#"{"type":"response.completed","response":{"status":"incomplete","usage":{"input_tokens":0,"output_tokens":0}}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert_eq!(events.len(), 1);
    if let StreamEvent::MessageEnd { message } = &events[0] {
        assert_eq!(message.stop_reason, StopReason::MaxTokens);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn openai_response_incomplete_event_maps_to_max_tokens_and_finishes() {
    let mut state = StreamState::new();
    let data = r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":7,"output_tokens":11}}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert!(state.finished);
    assert_eq!(events.len(), 1);
    if let StreamEvent::MessageEnd { message } = &events[0] {
        assert_eq!(message.stop_reason, StopReason::MaxTokens);
        let usage = message.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 11);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn openai_response_failed_event_finishes_with_error_reason() {
    let mut state = StreamState::new();
    let data = r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"upstream disconnected"}}}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let events = process_sse_event(event, &mut state);

    assert!(state.finished);
    assert_eq!(events.len(), 1);
    if let StreamEvent::MessageEnd { message } = &events[0] {
        assert_eq!(
            message.stop_reason,
            StopReason::Error("server_error: upstream disconnected".into())
        );
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn openai_reasoning_effort_off_returns_none() {
    assert!(reasoning_effort(ThinkingLevel::Off).is_none());
}

#[test]
fn openai_reasoning_effort_levels() {
    assert_eq!(
        reasoning_effort(ThinkingLevel::Minimal).as_deref(),
        Some("low")
    );
    assert_eq!(reasoning_effort(ThinkingLevel::Low).as_deref(), Some("low"));
    assert_eq!(
        reasoning_effort(ThinkingLevel::Medium).as_deref(),
        Some("medium")
    );
    assert_eq!(
        reasoning_effort(ThinkingLevel::High).as_deref(),
        Some("high")
    );
    assert_eq!(
        reasoning_effort(ThinkingLevel::XHigh).as_deref(),
        Some("xhigh")
    );
}

#[test]
fn openai_empty_instructions_omitted() {
    let model_meta = ModelMeta {
        id: "gpt-4o".into(),
        provider: "openai".into(),
        name: "GPT-4o".into(),
        context_window: 128_000,
        max_output_tokens: 16_384,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = OpenAiProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };
    let options = RequestOptions {
        system_prompt: "".into(),
        ..Default::default()
    };
    let req = build_request(&model, Context::default(), options);
    assert!(req.instructions.is_none());
    let json = serde_json::to_value(&req).unwrap();
    assert!(json.get("instructions").is_none());
}

#[test]
fn openai_default_max_output_tokens_are_capped() {
    let model_meta = ModelMeta {
        id: "gpt-5.4".into(),
        provider: "openai".into(),
        name: "GPT-5.4".into(),
        context_window: 400_000,
        max_output_tokens: 32_768,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = OpenAiProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };

    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert_eq!(req.max_output_tokens, Some(8_192));
}

#[test]
fn openai_explicit_max_output_tokens_override_cap() {
    let model_meta = ModelMeta {
        id: "gpt-5.4".into(),
        provider: "openai".into(),
        name: "GPT-5.4".into(),
        context_window: 400_000,
        max_output_tokens: 32_768,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = OpenAiProvider::new();
    let model = Model {
        meta: model_meta,
        provider: Arc::new(provider),
    };

    let req = build_request(
        &model,
        Context::default(),
        RequestOptions {
            max_tokens: Some(12_000),
            ..Default::default()
        },
    );
    assert_eq!(req.max_output_tokens, Some(12_000));
}

#[test]
fn openai_parse_sse_event_malformed_json_returns_error() {
    let result = parse_sse_event("{garbage}");
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::Stream(_)));
}

#[test]
fn openai_parse_sse_event_empty_returns_none() {
    let result = parse_sse_event("").unwrap();
    assert!(result.is_none());
}

#[test]
fn openai_unknown_event_type_ignored() {
    let data = r#"{"type":"response.in_progress"}"#;
    let event = parse_sse_event(data).unwrap().unwrap();
    let mut state = StreamState::new();
    let events = process_sse_event(event, &mut state);
    assert!(events.is_empty());
}
#[test]
fn openai_transport_capabilities_are_stateless_by_default() {
    assert!(!OpenAiProvider::persistent_transport_enabled_value(None));
    assert!(!OpenAiProvider::persistent_transport_enabled_value(Some(
        "0"
    )));

    let capabilities = TransportCapabilities::default();

    assert_eq!(capabilities, TransportCapabilities::default());
    assert_eq!(capabilities.persistent_session, PersistentSessionMode::None);
    assert_eq!(capabilities.continuation, ContinuationMode::None);
    assert_eq!(capabilities.resumability, ResumabilityMode::RestartRequest);
}

#[test]
fn openai_transport_capabilities_are_persistent_only_when_enabled() {
    for value in ["1", "true", "TRUE", "yes", "on"] {
        assert!(OpenAiProvider::persistent_transport_enabled_value(Some(
            value
        )));
    }

    let capabilities = OpenAiProvider::persistent_transport_capabilities();

    assert_eq!(
        capabilities.persistent_session,
        PersistentSessionMode::WebSocket
    );
    assert_eq!(
        capabilities.continuation,
        ContinuationMode::ProviderManagedId
    );
    assert_eq!(
        capabilities.resumability,
        ResumabilityMode::ResumeProviderState
    );
    assert!(capabilities.streaming);
}

#[test]
fn openai_websocket_payload_is_redacted_and_uses_create_event_type() {
    let model_meta = ModelMeta {
        id: "gpt-5.4".into(),
        provider: "openai".into(),
        name: "GPT-5.4".into(),
        context_window: 400_000,
        max_output_tokens: 32_768,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let model = Model {
        meta: model_meta,
        provider: Arc::new(OpenAiProvider::new()),
    };
    let request = build_request(&model, Context::default(), RequestOptions::default());
    let mut payload = serde_json::to_value(request).unwrap();
    if let serde_json::Value::Object(ref mut map) = payload {
        map.remove("stream");
        map.insert("store".to_string(), serde_json::Value::Bool(false));
        map.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
    }

    assert_eq!(payload["type"], "response.create");
    assert_eq!(payload["store"], false);
    assert!(payload.get("stream").is_none());
    let encoded = serde_json::to_string(&payload).unwrap();
    assert!(!encoded.contains("previous_response_id"));
    assert!(!encoded.contains("session_id"));
}
