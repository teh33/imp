use super::*;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::Provider;
use imp_llm::{AssistantMessage, RequestOptions, StopReason, StreamEvent, ToolResultMessage};

// -- helpers --

fn make_user(text: &str) -> Message {
    Message::user(text)
}

fn make_assistant_tool_call(call_id: &str, tool_name: &str, args: serde_json::Value) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: call_id.into(),
            name: tool_name.into(),
            arguments: args,
        }],
        usage: None,
        stop_reason: StopReason::ToolUse,
        timestamp: 1000,
    })
}

fn make_assistant_text(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text { text: text.into() }],
        usage: None,
        stop_reason: StopReason::EndTurn,
        timestamp: 1000,
    })
}

fn make_tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: call_id.into(),
        tool_name: tool_name.into(),
        content: vec![ContentBlock::Text {
            text: output.into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 1000,
    })
}

fn tool_result_text(msg: &Message) -> &str {
    match msg {
        Message::ToolResult(tr) => match &tr.content[0] {
            ContentBlock::Text { text } => text.as_str(),
            _ => panic!("expected text block"),
        },
        _ => panic!("expected ToolResult"),
    }
}

/// Minimal provider that never streams anything. Used for context_usage tests.
struct NullProvider;

#[async_trait]
impl Provider for NullProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: imp_llm::Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        Box::pin(futures::stream::empty())
    }

    async fn resolve_auth(
        &self,
        _auth: &imp_llm::auth::AuthStore,
    ) -> imp_llm::Result<imp_llm::auth::ApiKey> {
        Ok("test".into())
    }

    fn id(&self) -> &str {
        "null"
    }

    fn models(&self) -> &[ModelMeta] {
        &[]
    }
}

fn test_model() -> Model {
    Model {
        meta: ModelMeta {
            id: "test".into(),
            provider: "test".into(),
            name: "Test".into(),
            context_window: 100_000,
            max_output_tokens: 4096,
            pricing: ModelPricing::default(),
            capabilities: Capabilities::default(),
        },
        provider: Arc::new(NullProvider),
    }
}

// -- token estimation --

#[test]
fn estimate_tokens_rough_accuracy_for_english() {
    // "The quick brown fox jumps over the lazy dog" is 44 chars.
    // Real tokenizers produce ~10 tokens for this sentence.
    // Our estimate: 44 / 4 = 11. Within 2x of 10 ✓
    let text = "The quick brown fox jumps over the lazy dog";
    let est = estimate_tokens(text);
    let actual_approx = 10u32;
    assert!(
        est <= actual_approx * 2 && est * 2 >= actual_approx,
        "estimate {est} should be within 2x of ~{actual_approx}"
    );
}

#[test]
fn estimate_tokens_longer_text() {
    // ~400 chars of prose → ~100 tokens estimated, real is ~80–90.
    let text = "Rust is a multi-paradigm programming language designed for performance \
                    and safety, especially safe concurrency. Rust is syntactically similar to C++ \
                    but can guarantee memory safety by using a borrow checker to validate references. \
                    Rust achieves memory safety without garbage collection, and reference counting \
                    is optional. Rust was originally designed by Graydon Hoare at Mozilla Research.";
    let est = estimate_tokens(text);
    // ~380 chars / 4 = 95. Real ≈ 65-75 tokens. Ratio ≈ 1.3x — within 2x.
    assert!(est > 40 && est < 200, "estimate {est} out of range");
}

// -- observation masking --

#[test]
fn mask_observations_20_turns_keeps_last_10() {
    let mut messages = Vec::new();
    messages.push(make_user("initial prompt"));

    for i in 0..20 {
        let call_id = format!("call_{i}");
        messages.push(make_assistant_tool_call(
            &call_id,
            "read_file",
            serde_json::json!({"path": format!("/tmp/file_{i}.rs")}),
        ));
        messages.push(make_tool_result(
            &call_id,
            "read_file",
            &format!("Contents of file {i} — some long output here"),
        ));
    }
    // 1 user + 20*(assistant+tool_result) = 41 messages total

    mask_observations(&mut messages, 10);

    // First 10 turns are messages[1..21] — tool results at indices 2,4,6,...,20
    for i in 0..10 {
        let tr_idx = 2 + i * 2; // tool result indices: 2, 4, 6, ..., 20
        let text = tool_result_text(&messages[tr_idx]);
        assert!(
            text.starts_with("[Output omitted"),
            "Turn {i} tool result should be masked, got: {text}"
        );
    }

    // Last 10 turns are messages[21..41] — tool results at 22,24,...,40
    for i in 10..20 {
        let tr_idx = 2 + i * 2;
        let text = tool_result_text(&messages[tr_idx]);
        assert!(
            text.starts_with("Contents of file"),
            "Turn {i} tool result should be intact, got: {text}"
        );
    }
}

#[test]
fn masking_preserves_user_messages() {
    let mut messages = Vec::new();
    messages.push(make_user("Hello, help me with this task"));

    for i in 0..5 {
        let call_id = format!("call_{i}");
        messages.push(make_assistant_tool_call(
            &call_id,
            "bash",
            serde_json::json!({"command": format!("ls /tmp/{i}")}),
        ));
        messages.push(make_tool_result(
            &call_id,
            "bash",
            &format!("file_{i}.txt\nmore_output_{i}"),
        ));
    }

    mask_observations(&mut messages, 2);

    // User message at index 0 must be preserved verbatim.
    if let Message::User(u) = &messages[0] {
        if let ContentBlock::Text { text } = &u.content[0] {
            assert_eq!(text, "Hello, help me with this task");
        } else {
            panic!("expected Text block in user message");
        }
    } else {
        panic!("expected User message at index 0");
    }
}

#[test]
fn masking_preserves_assistant_text_and_tool_call_args() {
    let mut messages = Vec::new();
    messages.push(make_user("do stuff"));

    for i in 0..4 {
        let call_id = format!("call_{i}");
        let args = serde_json::json!({"command": format!("echo {i}")});
        messages.push(make_assistant_tool_call(&call_id, "bash", args));
        messages.push(make_tool_result(&call_id, "bash", &format!("output {i}")));
    }
    messages.push(make_assistant_text("All done!"));

    // Keep last 1 turn (the final text-only assistant). That means 4 tool turns get masked.
    mask_observations(&mut messages, 1);

    // Check all assistant messages are fully preserved.
    for msg in &messages {
        if let Message::Assistant(a) = msg {
            for block in &a.content {
                match block {
                    ContentBlock::ToolCall {
                        name, arguments, ..
                    } => {
                        assert_eq!(name, "bash");
                        assert!(arguments.get("command").is_some());
                    }
                    ContentBlock::Text { text } => {
                        assert_eq!(text, "All done!");
                    }
                    _ => {}
                }
            }
        }
    }

    // Tool results in old turns are masked but preserve tool_call_id, tool_name, is_error.
    let tool_results: Vec<&ToolResultMessage> = messages
        .iter()
        .filter_map(|m| {
            if let Message::ToolResult(tr) = m {
                Some(tr)
            } else {
                None
            }
        })
        .collect();

    for tr in &tool_results {
        assert_eq!(tr.tool_name, "bash");
        assert!(!tr.is_error);
        assert!(!tr.tool_call_id.is_empty());
    }
}

#[test]
fn mask_observations_includes_args_summary() {
    let mut messages = Vec::new();
    messages.push(make_user("do stuff"));

    let args = serde_json::json!({"path": "/src/main.rs", "line": 42});
    messages.push(make_assistant_tool_call("c1", "read_file", args));
    messages.push(make_tool_result("c1", "read_file", "fn main() {}"));

    messages.push(make_assistant_text("done"));

    // Keep only the last turn (text-only), so the tool turn gets masked.
    mask_observations(&mut messages, 1);

    let text = tool_result_text(&messages[2]);
    assert!(text.contains("read_file"), "should contain tool name");
    assert!(text.contains("/src/main.rs"), "should contain args summary");
    assert!(text.contains("bytes"), "should contain byte count");
}

#[test]
fn mask_observations_handles_multibyte_args_without_panicking() {
    let mut messages = vec![make_user("do stuff")];

    let long_text = format!("{}—bbb", "a".repeat(86));
    messages.push(make_assistant_tool_call(
        "c1",
        "edit",
        serde_json::json!({"newText": long_text}),
    ));
    messages.push(make_tool_result("c1", "edit", "ok"));
    messages.push(make_assistant_text("done"));

    mask_observations(&mut messages, 1);

    let text = tool_result_text(&messages[2]);
    assert!(text.starts_with("[Output omitted"));
    assert!(text.contains("..."));
}

#[test]
fn mask_observations_noop_when_few_turns() {
    let mut messages = vec![make_user("hi"), make_assistant_text("hello")];
    let original = messages.clone();

    mask_observations(&mut messages, 10);

    // Nothing should change — only 1 turn, window is 10.
    assert_eq!(messages.len(), original.len());
}

// -- context usage --

#[test]
fn context_usage_basic_calculation() {
    let model = test_model();
    let messages = vec![make_user("Hello world"), make_assistant_text("Hi there!")];

    let usage = context_usage(&messages, &model);

    assert!(usage.used > 0, "should estimate > 0 tokens");
    assert_eq!(usage.limit, 100_000);
    assert!(usage.ratio > 0.0, "ratio should be positive");
    assert!(usage.ratio < 1.0, "ratio should be < 1 for small messages");
}

#[test]
fn context_usage_masked_vs_unmasked() {
    let model = test_model();

    let mut messages = Vec::new();
    messages.push(make_user("prompt"));
    for i in 0..10 {
        let call_id = format!("c{i}");
        let big_output = "x".repeat(2000);
        messages.push(make_assistant_tool_call(
            &call_id,
            "bash",
            serde_json::json!({"cmd": "ls"}),
        ));
        messages.push(make_tool_result(&call_id, "bash", &big_output));
    }

    let usage_before = context_usage(&messages, &model);

    mask_observations(&mut messages, 2);

    let usage_after = context_usage(&messages, &model);

    assert!(
        usage_after.used < usage_before.used,
        "masking should reduce token count: before={}, after={}",
        usage_before.used,
        usage_after.used
    );
}

#[test]
fn gpt_5_5_context_budget_uses_one_million_display_window_with_buffer() {
    let mut model = test_model();
    model.meta.id = "gpt-5.5".into();
    model.meta.context_window = 1_050_000;

    let budget = context_budget(&model);

    assert_eq!(budget.total_window, 1_050_000);
    assert_eq!(budget.display_window, 1_000_000);
    assert_eq!(budget.reserved_buffer, 50_000);
    assert_eq!(budget.ratio_for_used(500_000), 0.5);
}

#[test]
fn non_gpt_5_5_context_budget_uses_model_window_without_buffer() {
    let model = test_model();

    let budget = context_budget(&model);

    assert_eq!(budget.total_window, 100_000);
    assert_eq!(budget.display_window, 100_000);
    assert_eq!(budget.reserved_buffer, 0);
}

#[test]
fn context_usage_uses_budget_display_window() {
    let mut model = test_model();
    model.meta.id = "gpt-5.5".into();
    model.meta.context_window = 1_050_000;
    let messages = vec![make_user(&"x".repeat(4_000))];

    let usage = context_usage(&messages, &model);

    assert_eq!(usage.limit, 1_000_000);
    assert!(usage.used > 0);
    assert_eq!(usage.ratio, usage.used as f64 / 1_000_000.0);
}

#[test]
fn openai_text_estimation_uses_tokenizer_for_gpt_models() {
    let mut model = test_model();
    model.meta.id = "gpt-5.5".into();
    model.meta.provider = "openai-codex".into();

    let text = "hello world";
    let estimated = estimate_text_tokens_for_model(text, &model.meta);

    assert_eq!(estimated, 2);
}

#[test]
fn non_openai_text_estimation_uses_rough_fallback() {
    let model = test_model();
    let text = "hello world";

    assert_eq!(
        estimate_text_tokens_for_model(text, &model.meta),
        estimate_tokens(text)
    );
}

#[test]
fn openai_message_estimation_includes_message_overhead() {
    let mut model = test_model();
    model.meta.id = "gpt-5.5".into();
    model.meta.provider = "openai-codex".into();
    let message = make_user("hello world");

    let estimated = estimate_message_tokens_for_model(&message, &model.meta);

    assert!(estimated > estimate_text_tokens_for_model("hello world", &model.meta));
}

// -- edge case tests --

#[test]
fn estimate_tokens_empty_string() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn context_usage_with_zero_messages() {
    let model = test_model();
    let messages: Vec<Message> = vec![];

    let usage = context_usage(&messages, &model);

    assert_eq!(usage.used, 0);
    assert_eq!(usage.ratio, 0.0);
    assert_eq!(usage.limit, 100_000);
}

#[test]
fn context_usage_near_limit() {
    // Create a message with enough text to approach the limit.
    let big_text = "a".repeat(400);
    let messages = vec![make_user(&big_text)];

    // Compute estimated tokens for this message, then set context_window = estimated + 1
    // so ratio is just under 1.0.
    let json = serde_json::to_string(&messages[0]).unwrap();
    let estimated = estimate_tokens(&json);
    let window = estimated + 1;

    let model = Model {
        meta: ModelMeta {
            id: "test".into(),
            provider: "test".into(),
            name: "Test".into(),
            context_window: window,
            max_output_tokens: 4096,
            pricing: ModelPricing::default(),
            capabilities: Capabilities::default(),
        },
        provider: Arc::new(NullProvider),
    };

    let usage = context_usage(&messages, &model);

    assert!(usage.ratio > 0.95, "ratio {} should be > 0.95", usage.ratio);
    assert!(usage.ratio < 1.0, "ratio {} should be < 1.0", usage.ratio);
}

#[test]
fn mask_observations_replaces_content_with_placeholder() {
    let mut messages = vec![make_user("prompt")];
    let args = serde_json::json!({"path": "/src/lib.rs"});
    messages.push(make_assistant_tool_call("c1", "read_file", args));
    messages.push(make_tool_result(
        "c1",
        "read_file",
        "fn main() { println!(\"hello\"); }",
    ));
    // Second turn stays recent.
    messages.push(make_assistant_text("Done reading."));

    // Keep only last 1 turn → the tool turn gets masked.
    mask_observations(&mut messages, 1);

    let text = tool_result_text(&messages[2]);
    // Verify exact placeholder format.
    assert!(
        text.starts_with("[Output omitted — ran read_file("),
        "placeholder should start correctly, got: {text}"
    );
    assert!(
        text.contains("/src/lib.rs"),
        "placeholder should contain args summary, got: {text}"
    );
    assert!(
        text.ends_with("bytes]"),
        "placeholder should end with byte count, got: {text}"
    );
    // Verify byte count matches original content length.
    let original_len = "fn main() { println!(\"hello\"); }".len();
    assert!(
        text.contains(&format!("{original_len} bytes")),
        "placeholder should contain correct byte count {original_len}, got: {text}"
    );
}

#[test]
fn mask_observations_preserves_all_assistant_reasoning() {
    let mut messages = vec![make_user("help me refactor")];

    // Turn 0: assistant text + tool call.
    messages.push(Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text {
                text: "Let me read the file first.".into(),
            },
            ContentBlock::ToolCall {
                id: "c0".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
            },
        ],
        usage: None,
        stop_reason: StopReason::ToolUse,
        timestamp: 1000,
    }));
    messages.push(make_tool_result("c0", "read", "file contents A"));

    // Turn 1: assistant reasoning text.
    messages.push(make_assistant_text(
        "I see the issue — the struct is missing a field.",
    ));

    // Turn 2: another tool call.
    messages.push(make_assistant_tool_call(
        "c2",
        "edit",
        serde_json::json!({"file": "a.rs"}),
    ));
    messages.push(make_tool_result("c2", "edit", "ok"));

    // Keep last 1 turn → turns 0 and 1 get masked.
    mask_observations(&mut messages, 1);

    // Collect ALL assistant text blocks — they should all be intact.
    let assistant_texts: Vec<&str> = messages
        .iter()
        .filter_map(|m| {
            if let Message::Assistant(a) = m {
                Some(a.content.iter().filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                }))
            } else {
                None
            }
        })
        .flatten()
        .collect();

    assert!(
        assistant_texts.contains(&"Let me read the file first."),
        "early assistant reasoning must survive masking"
    );
    assert!(
        assistant_texts.contains(&"I see the issue — the struct is missing a field."),
        "mid-conversation assistant reasoning must survive masking"
    );
}
