use super::*;
use crate::message::UserMessage;

fn test_model(id: &str) -> Model {
    let provider = GoogleProvider::new();
    Model {
        meta: builtin_models()
            .into_iter()
            .find(|meta| meta.id == id)
            .expect("test model should exist"),
        provider: provider.into_arc(),
    }
}

#[test]
fn serialize_text_user_message() {
    let message = Message::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "Hello Gemini".into(),
        }],
        timestamp: 0,
    });

    let api = convert_message(&message);
    let json = serde_json::to_value(&api).unwrap();

    assert_eq!(json["role"], "user");
    assert_eq!(json["parts"][0]["text"], "Hello Gemini");
}

#[test]
fn serialize_assistant_tool_call_block() {
    let message = Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        }],
        usage: None,
        stop_reason: StopReason::ToolUse,
        timestamp: 0,
    });

    let api = convert_message(&message);
    let json = serde_json::to_value(&api).unwrap();

    assert_eq!(json["role"], "model");
    assert_eq!(json["parts"][0]["functionCall"]["id"], "call_1");
    assert_eq!(json["parts"][0]["functionCall"]["name"], "bash");
    assert_eq!(json["parts"][0]["functionCall"]["args"]["command"], "ls");
}

#[test]
fn serialize_tool_result_message() {
    let message = Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "README.md\nsrc/".into(),
        }],
        is_error: false,
        details: serde_json::json!({"cwd": "/tmp"}),
        timestamp: 0,
    });

    let api = convert_message(&message);
    let json = serde_json::to_value(&api).unwrap();

    assert_eq!(json["role"], "user");
    assert_eq!(json["parts"][0]["functionResponse"]["id"], "call_1");
    assert_eq!(json["parts"][0]["functionResponse"]["name"], "bash");
    assert_eq!(
        json["parts"][0]["functionResponse"]["response"]["result"],
        "README.md\nsrc/"
    );
    assert_eq!(
        json["parts"][0]["functionResponse"]["response"]["details"]["cwd"],
        "/tmp"
    );
}

#[test]
fn thinking_budget_mapping_matches_model_limits() {
    let pro = test_model("gemini-2.5-pro");
    let flash = test_model("gemini-2.5-flash");

    assert_eq!(thinking_budget(&pro, ThinkingLevel::Off), None);
    assert_eq!(thinking_budget(&pro, ThinkingLevel::Minimal), Some(1024));
    assert_eq!(thinking_budget(&pro, ThinkingLevel::Low), Some(4096));
    assert_eq!(thinking_budget(&pro, ThinkingLevel::Medium), Some(10_000));
    assert_eq!(thinking_budget(&pro, ThinkingLevel::High), Some(24_576));
    assert_eq!(thinking_budget(&pro, ThinkingLevel::XHigh), Some(32_768));
    assert_eq!(thinking_budget(&flash, ThinkingLevel::XHigh), Some(24_576));
}

#[test]
fn default_max_output_tokens_caps_google_models_without_thinking() {
    let pro = test_model("gemini-2.5-pro");
    assert_eq!(default_max_output_tokens(&pro, None), 8_192);
}

#[test]
fn default_max_output_tokens_grows_for_google_thinking_budget() {
    let pro = test_model("gemini-2.5-pro");
    assert_eq!(default_max_output_tokens(&pro, Some(24_576)), 25_600);
}

#[test]
fn build_request_serializes_system_tools_and_thinking() {
    let model = test_model("gemini-2.5-pro");
    let context = Context {
        messages: vec![
            Message::user("List the files in this directory."),
            Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                }],
                usage: None,
                stop_reason: StopReason::ToolUse,
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1".into(),
                tool_name: "bash".into(),
                content: vec![ContentBlock::Text {
                    text: "Cargo.toml\nsrc/".into(),
                }],
                is_error: false,
                details: serde_json::Value::Null,
                timestamp: 0,
            }),
        ],
        ..Default::default()
    };
    let options = RequestOptions {
        system_prompt: "You are a helpful coding assistant.".into(),
        max_tokens: Some(2048),
        temperature: Some(0.2),
        thinking_level: ThinkingLevel::High,
        tools: vec![ToolDefinition {
            name: "bash".into(),
            description: "Run a shell command".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
        }],
        ..Default::default()
    };

    let request = build_request(&model, context, options);
    let json = serde_json::to_value(&request).unwrap();

    assert_eq!(
        json["systemInstruction"]["parts"][0]["text"],
        "You are a helpful coding assistant."
    );
    assert_eq!(json["contents"].as_array().unwrap().len(), 3);
    assert_eq!(json["contents"][0]["role"], "user");
    assert_eq!(json["contents"][1]["role"], "model");
    assert_eq!(
        json["contents"][1]["parts"][0]["functionCall"]["name"],
        "bash"
    );
    assert_eq!(
        json["contents"][2]["parts"][0]["functionResponse"]["name"],
        "bash"
    );
    assert_eq!(json["tools"][0]["functionDeclarations"][0]["name"], "bash");
    assert_eq!(json["generationConfig"]["maxOutputTokens"], 2048);
    assert!(
        (json["generationConfig"]["temperature"]
            .as_f64()
            .expect("temperature should be numeric")
            - 0.2)
            .abs()
            < 1e-6
    );
    assert_eq!(
        json["generationConfig"]["thinkingConfig"]["includeThoughts"],
        true
    );
    assert_eq!(
        json["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        24_576
    );
}

#[test]
fn parse_text_and_thinking_deltas() {
    let raw = "\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"Plan\"}]}}]}\n\
 \n\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"Planning\"},{\"text\":\"Answer\"}]}}]}\n\
 \n\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"Planning\"},{\"text\":\"Answer done\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"thoughtsTokenCount\":3}}\n";

    let mut state = StreamState::new("gemini-2.5-pro".into());
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    assert!(matches!(&events[0], StreamEvent::MessageStart { model } if model == "gemini-2.5-pro"));
    assert!(matches!(&events[1], StreamEvent::ThinkingDelta { text } if text == "Plan"));
    assert!(matches!(&events[2], StreamEvent::ThinkingDelta { text } if text == "ning"));
    assert!(matches!(&events[3], StreamEvent::TextDelta { text } if text == "Answer"));
    assert!(matches!(&events[4], StreamEvent::TextDelta { text } if text == " done"));
    assert!(
        matches!(&events[5], StreamEvent::MessageEnd { message } if message.stop_reason == StopReason::EndTurn)
    );

    if let StreamEvent::MessageEnd { message } = &events[5] {
        assert_eq!(message.usage.as_ref().unwrap().input_tokens, 10);
        assert_eq!(message.usage.as_ref().unwrap().output_tokens, 8);
        assert_eq!(message.content.len(), 2);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn parse_tool_call_response() {
    let raw = "\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"id\":\"call_1\",\"name\":\"read\",\"args\":{\"path\":\"src/lib.rs\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":4}}\n";

    let mut state = StreamState::new("gemini-2.5-pro".into());
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], StreamEvent::MessageStart { .. }));
    assert!(
        matches!(&events[1], StreamEvent::ToolCall { id, name, arguments } if id == "call_1" && name == "read" && arguments["path"] == "src/lib.rs")
    );
    assert!(
        matches!(&events[2], StreamEvent::MessageEnd { message } if message.stop_reason == StopReason::ToolUse)
    );
}

#[test]
fn parse_invalid_sse_event_returns_error() {
    let error = parse_sse_event("not json").unwrap_err();
    assert!(matches!(error, Error::Stream(_)));
}

#[test]
fn builtin_models_include_flash_and_pro() {
    let models = builtin_models();
    assert_eq!(models.len(), 2);
    assert!(models.iter().any(|model| model.id == "gemini-2.5-pro"));
    assert!(models.iter().any(|model| model.id == "gemini-2.5-flash"));
}

#[test]
fn parse_multi_part_response_text_and_tool_call() {
    // A single candidate with both text and a function_call in the same response
    let raw = "\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Let me check\"},{\"functionCall\":{\"id\":\"call_1\",\"name\":\"read\",\"args\":{\"path\":\"a.rs\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":8,\"candidatesTokenCount\":6}}\n";

    let mut state = StreamState::new("gemini-2.5-pro".into());
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    // MessageStart, TextDelta, ToolCall, MessageEnd
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(&events[1], StreamEvent::TextDelta { text } if text == "Let me check"));
    assert!(matches!(&events[2], StreamEvent::ToolCall { name, .. } if name == "read"));
    if let StreamEvent::MessageEnd { message } = &events[3] {
        assert_eq!(message.stop_reason, StopReason::ToolUse);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn parse_usage_metadata_extraction() {
    let raw = "\
 data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hi\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":42,\"candidatesTokenCount\":10,\"thoughtsTokenCount\":5,\"cachedContentTokenCount\":3}}\n";

    let mut state = StreamState::new("gemini-2.5-pro".into());
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    if let StreamEvent::MessageEnd { message } = events.last().unwrap() {
        let usage = message.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 42);
        assert_eq!(usage.output_tokens, 15); // candidates + thoughts
        assert_eq!(usage.cache_read_tokens, 3);
    } else {
        panic!("expected MessageEnd");
    }
}

#[test]
fn stop_reason_mapping() {
    let mut state = StreamState::new("test".into());
    state.finish_reason = Some("STOP".into());
    assert_eq!(state.stop_reason(), StopReason::EndTurn);

    state.finish_reason = Some("MAX_TOKENS".into());
    assert_eq!(state.stop_reason(), StopReason::MaxTokens);

    state.finish_reason = Some("SAFETY".into());
    assert_eq!(state.stop_reason(), StopReason::Error("SAFETY".into()));

    state.finish_reason = None;
    assert_eq!(state.stop_reason(), StopReason::EndTurn);

    state.saw_tool_call = true;
    assert_eq!(state.stop_reason(), StopReason::ToolUse);
}

#[test]
fn empty_candidates_produces_no_content_events() {
    let raw = "\
 data: {\"candidates\":[],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":0}}\n";

    let mut state = StreamState::new("gemini-2.5-pro".into());
    let events = parse_sse_stream(raw, &mut state);
    let events: Vec<_> = events
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    // Only MessageStart (no content deltas, no MessageEnd since no finishReason)
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::MessageStart { .. }));
}

#[test]
fn parse_sse_event_done_marker_returns_none() {
    let result = parse_sse_event("[DONE]").unwrap();
    assert!(result.is_none());
}

#[test]
fn empty_system_prompt_produces_no_instruction() {
    let instruction = build_system_instruction("");
    assert!(instruction.is_none());
}

#[test]
fn empty_tools_produces_empty_vec() {
    let tools = build_tools(&[]);
    assert!(tools.is_empty());
}
