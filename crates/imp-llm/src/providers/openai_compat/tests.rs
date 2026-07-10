use std::sync::Arc;

use super::*;
use crate::message::{AssistantMessage, ToolResultMessage, UserMessage};
use crate::model::{Capabilities, ModelPricing};
use crate::provider::{Context, RequestOptions};

fn test_model() -> Model {
    test_model_for_provider("deepseek-chat", "deepseek")
}

fn test_model_for_provider(id: &str, provider_id: &str) -> Model {
    let meta = ModelMeta {
        id: id.into(),
        provider: provider_id.into(),
        name: id.into(),
        context_window: 64_000,
        max_output_tokens: 4_096,
        pricing: ModelPricing::default(),
        capabilities: Capabilities {
            reasoning: false,
            images: false,
            tool_use: true,
        },
    };
    let provider =
        OpenAiCompatProvider::new(provider_id, "https://api.example.com", vec![meta.clone()]);
    Model {
        meta,
        provider: Arc::new(provider),
    }
}

// -- build_request tests --

#[test]
fn openai_compat_system_prompt_becomes_system_message() {
    let model = test_model();
    let context = Context {
        messages: vec![],
        ..Default::default()
    };
    let options = RequestOptions {
        system_prompt: "You are a helpful assistant.".into(),
        ..Default::default()
    };

    let req = build_request(&model, context, options);

    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, "system");
    assert_eq!(
        req.messages[0].content,
        Some(serde_json::Value::String(
            "You are a helpful assistant.".into()
        ))
    );
}

#[test]
fn openai_compat_empty_system_prompt_omitted() {
    let model = test_model();
    let options = RequestOptions {
        system_prompt: "".into(),
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    assert!(req.messages.is_empty());
}

#[test]
fn openai_compat_user_text_message() {
    let model = test_model();
    let context = Context {
        messages: vec![Message::user("Hello!")],
        ..Default::default()
    };
    let options = RequestOptions::default();

    let req = build_request(&model, context, options);

    assert_eq!(req.messages.len(), 1);
    assert_eq!(req.messages[0].role, "user");
    assert_eq!(
        req.messages[0].content,
        Some(serde_json::Value::String("Hello!".into()))
    );
}

#[test]
fn openai_compat_user_message_with_image() {
    let model = test_model();
    let context = Context {
        messages: vec![Message::User(UserMessage {
            content: vec![
                ContentBlock::Text {
                    text: "What is this?".into(),
                },
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "abc123".into(),
                },
            ],
            timestamp: 0,
        })],
        ..Default::default()
    };

    let req = build_request(&model, context, RequestOptions::default());

    let content = req.messages[0].content.as_ref().unwrap();
    let arr = content.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "What is this?");
    assert_eq!(arr[1]["type"], "image_url");
    assert_eq!(arr[1]["image_url"]["url"], "data:image/png;base64,abc123");
}

#[test]
fn openai_compat_assistant_with_tool_call() {
    let msg = Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "Running bash.".into(),
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
    });

    let converted = convert_message(&msg);
    assert_eq!(converted.len(), 1);
    let api_msg = &converted[0];
    assert_eq!(api_msg.role, "assistant");
    assert_eq!(
        api_msg.content,
        Some(serde_json::Value::String("Running bash.".into()))
    );
    let tcs = api_msg.tool_calls.as_ref().unwrap();
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].id, "call_1");
    assert_eq!(tcs[0].function.name, "bash");
    assert_eq!(tcs[0].function.arguments, r#"{"command":"ls"}"#);
}

#[test]
fn openai_compat_tool_result_message() {
    let msg = Message::ToolResult(ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "README.md\nsrc/".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    });

    let converted = convert_message(&msg);
    assert_eq!(converted.len(), 1);
    let api_msg = &converted[0];
    assert_eq!(api_msg.role, "tool");
    assert_eq!(api_msg.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(
        api_msg.content,
        Some(serde_json::Value::String("README.md\nsrc/".into()))
    );
}

#[test]
fn openai_compat_tool_definitions() {
    let tools = vec![crate::provider::ToolDefinition {
        name: "read_file".into(),
        description: "Read a file from disk".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        }),
    }];

    let defs = build_tool_defs(&tools);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].tool_type, "function");
    assert_eq!(defs[0].function.name, "read_file");
    assert_eq!(defs[0].function.description, "Read a file from disk");
    assert_eq!(defs[0].function.parameters["type"], "object");
}

#[test]
fn openai_compat_temperature_included_when_set() {
    let model = test_model();
    let options = RequestOptions {
        temperature: Some(0.7),
        ..Default::default()
    };

    let req = build_request(&model, Context::default(), options);
    assert_eq!(req.temperature, Some(0.7));
}

#[test]
fn kimi_fixed_temperature_models_omit_temperature() {
    let model = test_model_for_provider("kimi-k2.6", "moonshot");
    let req = build_request(
        &model,
        Context::default(),
        RequestOptions {
            temperature: Some(0.7),
            ..Default::default()
        },
    );

    assert_eq!(req.temperature, None);
}

#[test]
fn kimi_request_sends_thinking_control() {
    let model = test_model_for_provider("kimi-k2.6", "moonshot");
    let enabled = build_request(
        &model,
        Context::default(),
        RequestOptions {
            thinking_level: crate::provider::ThinkingLevel::Medium,
            ..Default::default()
        },
    );
    let disabled = build_request(
        &model,
        Context::default(),
        RequestOptions {
            thinking_level: crate::provider::ThinkingLevel::Off,
            ..Default::default()
        },
    );

    let enabled_json = serde_json::to_value(&enabled).unwrap();
    assert_eq!(enabled_json["thinking"]["type"], "enabled");
    assert_eq!(enabled_json["thinking"]["keep"], "all");

    let disabled_json = serde_json::to_value(&disabled).unwrap();
    assert_eq!(disabled_json["thinking"]["type"], "disabled");
    assert!(disabled_json["thinking"].get("keep").is_none());
}

#[test]
fn kimi_legacy_preview_omits_thinking_control() {
    let model = test_model_for_provider("kimi-k2-turbo-preview", "moonshot");
    let req = build_request(
        &model,
        Context::default(),
        RequestOptions {
            thinking_level: crate::provider::ThinkingLevel::Medium,
            ..Default::default()
        },
    );
    let json = serde_json::to_value(&req).unwrap();

    assert!(json.get("thinking").is_none());
}

#[test]
fn kimi_forced_thinking_models_keep_thinking_enabled() {
    let model = test_model_for_provider("kimi-k2-thinking", "moonshot");
    let req = build_request(
        &model,
        Context::default(),
        RequestOptions {
            thinking_level: crate::provider::ThinkingLevel::Off,
            ..Default::default()
        },
    );
    let json = serde_json::to_value(&req).unwrap();

    assert_eq!(json["thinking"]["type"], "enabled");
    assert_eq!(json["thinking"]["keep"], "all");
}

#[test]
fn kimi_k2_7_forced_thinking_always_enabled() {
    for model_id in ["kimi-k2.7-code", "kimi-k2.7-code-highspeed"] {
        let model = test_model_for_provider(model_id, "moonshot");
        let req = build_request(
            &model,
            Context::default(),
            RequestOptions {
                thinking_level: crate::provider::ThinkingLevel::Off,
                temperature: Some(0.7),
                ..Default::default()
            },
        );
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["thinking"]["type"], "enabled", "{model_id}");
        assert_eq!(json["thinking"]["keep"], "all", "{model_id}");
        assert!(json["temperature"].is_null(), "{model_id}");
    }
}

#[test]
fn kimi_code_k2_7_maps_to_api_model_id() {
    let model = test_model_for_provider("kimi2.7", "kimi-code");
    let req = build_request(&model, Context::default(), RequestOptions::default());
    let json = serde_json::to_value(&req).unwrap();

    assert_eq!(json["model"], "kimi-k2.7-code");
}

#[test]
fn openai_compat_preserves_assistant_reasoning_content() {
    let msg = Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Thinking {
                text: "reasoning".into(),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ],
        usage: None,
        stop_reason: StopReason::EndTurn,
        timestamp: 0,
    });

    let converted = convert_message(&msg);
    assert_eq!(converted.len(), 1);
    let json = serde_json::to_value(&converted[0]).unwrap();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["reasoning_content"], "reasoning");
    assert_eq!(json["content"], "answer");
}

#[test]
fn openai_compat_temperature_omitted_when_none() {
    let model = test_model();
    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert!(req.temperature.is_none());
}

#[test]
fn openai_compat_max_tokens_falls_back_to_provider_default_cap() {
    let model = test_model();
    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert_eq!(req.max_tokens, Some(4_096));
}

#[test]
fn openai_compat_large_model_default_max_tokens_are_capped() {
    let meta = ModelMeta {
        id: "large-compat".into(),
        provider: "deepseek".into(),
        name: "Large Compat".into(),
        context_window: 128_000,
        max_output_tokens: 32_768,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider =
        OpenAiCompatProvider::new("deepseek", "https://api.deepseek.com", vec![meta.clone()]);
    let model = Model {
        meta,
        provider: Arc::new(provider),
    };

    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert_eq!(req.max_tokens, Some(8_192));
}

#[test]
fn openai_compat_explicit_max_tokens_override_cap() {
    let meta = ModelMeta {
        id: "large-compat".into(),
        provider: "deepseek".into(),
        name: "Large Compat".into(),
        context_window: 128_000,
        max_output_tokens: 32_768,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider =
        OpenAiCompatProvider::new("deepseek", "https://api.deepseek.com", vec![meta.clone()]);
    let model = Model {
        meta,
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
    assert_eq!(req.max_tokens, Some(12_000));
}

#[test]
fn openai_compat_stream_options_always_include_usage() {
    let model = test_model();
    let req = build_request(&model, Context::default(), RequestOptions::default());
    assert!(req.stream_options.include_usage);
}

#[test]
fn openai_compat_request_serializes_correctly() {
    let model = test_model();
    let context = Context {
        messages: vec![Message::user("Hi")],
        ..Default::default()
    };
    let options = RequestOptions {
        system_prompt: "Be helpful.".into(),
        temperature: Some(0.5),
        ..Default::default()
    };

    let req = build_request(&model, context, options);
    let json = serde_json::to_value(&req).unwrap();

    assert_eq!(json["model"], "deepseek-chat");
    assert!(json["stream"].as_bool().unwrap());
    assert_eq!(json["stream_options"]["include_usage"], true);
    assert_eq!(json["temperature"], 0.5);

    let messages = json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "Be helpful.");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Hi");
}

// -- SSE parsing tests --

#[test]
fn openai_compat_parse_text_chunk() {
    let data = r#"{"choices":[{"delta":{"content":"Hello"},"index":0,"finish_reason":null}]}"#;
    let chunk = parse_sse_chunk(data).unwrap().unwrap();
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
}

#[test]
fn openai_compat_parse_done_returns_none() {
    let result = parse_sse_chunk("[DONE]").unwrap();
    assert!(result.is_none());
}

#[test]
fn openai_compat_parse_empty_returns_none() {
    let result = parse_sse_chunk("").unwrap();
    assert!(result.is_none());
}

#[test]
fn openai_compat_parse_malformed_returns_error() {
    let result = parse_sse_chunk("{bad json}");
    assert!(result.is_err());
}

#[test]
fn openai_compat_parse_tool_call_delta() {
    let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"bash","arguments":""}}]},"index":0,"finish_reason":null}]}"#;
    let chunk = parse_sse_chunk(data).unwrap().unwrap();
    let tcs = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
    assert_eq!(tcs.len(), 1);
    assert_eq!(tcs[0].index, 0);
    assert_eq!(tcs[0].id.as_deref(), Some("call_abc"));
    assert_eq!(
        tcs[0].function.as_ref().unwrap().name.as_deref(),
        Some("bash")
    );
}

#[test]
fn openai_compat_parse_usage_chunk() {
    let data =
        r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
    let chunk = parse_sse_chunk(data).unwrap().unwrap();
    let usage = chunk.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 20);
}

#[test]
fn openai_compat_parse_reasoning_content() {
    let data = r#"{"choices":[{"delta":{"reasoning_content":"Let me think...","content":null},"index":0,"finish_reason":null}]}"#;
    let chunk = parse_sse_chunk(data).unwrap().unwrap();
    assert_eq!(
        chunk.choices[0].delta.reasoning_content.as_deref(),
        Some("Let me think...")
    );
    assert!(chunk.choices[0].delta.content.is_none());
}

#[test]
fn openai_compat_provider_id() {
    let provider = OpenAiCompatProvider::new("deepseek", "https://api.deepseek.com", vec![]);
    assert_eq!(provider.id(), "deepseek");
}

#[test]
fn openai_compat_provider_models() {
    let meta = ModelMeta {
        id: "deepseek-chat".into(),
        provider: "deepseek".into(),
        name: "DeepSeek Chat".into(),
        context_window: 64_000,
        max_output_tokens: 4_096,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    let provider = OpenAiCompatProvider::new("deepseek", "https://api.deepseek.com", vec![meta]);
    assert_eq!(provider.models().len(), 1);
    assert_eq!(provider.models()[0].id, "deepseek-chat");
}
