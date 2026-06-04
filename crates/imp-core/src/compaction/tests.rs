use super::*;
use crate::session::SessionManager;
use async_trait::async_trait;
use futures_core::Stream;
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::Provider;
use imp_llm::{
    AssistantMessage, Model, RequestOptions, StopReason, StreamEvent, ToolResultMessage,
};
use std::pin::Pin;
use std::sync::Arc;

#[test]
fn compaction_strategy_defaults_to_local() {
    let caps = CompactionCapabilities {
        provider_id: "anthropic",
        model_id: "claude-sonnet",
        allow_provider_native: false,
    };
    assert_eq!(select_compaction_strategy(&caps), CompactionStrategy::Local);
}

#[test]
fn compaction_strategy_exposes_provider_native_seam_for_supported_providers() {
    let openai = CompactionCapabilities {
        provider_id: "openai-codex",
        model_id: "gpt-5-codex",
        allow_provider_native: true,
    };
    assert_eq!(
        select_compaction_strategy(&openai),
        CompactionStrategy::ProviderNative
    );

    let anthropic = CompactionCapabilities {
        provider_id: "anthropic",
        model_id: "claude-sonnet-4-5",
        allow_provider_native: true,
    };
    assert_eq!(
        select_compaction_strategy(&anthropic),
        CompactionStrategy::ProviderNative
    );
}

#[test]
fn compaction_strategy_keeps_unknown_providers_local() {
    let caps = CompactionCapabilities {
        provider_id: "deepseek",
        model_id: "deepseek-chat",
        allow_provider_native: true,
    };
    assert_eq!(select_compaction_strategy(&caps), CompactionStrategy::Local);
}

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

#[test]
fn summary_prompt_options_use_custom_prompt_and_target_tokens() {
    let messages = vec![
        make_user("Please preserve src/main.rs and the failing cargo test."),
        make_assistant_tool_call(
            "c1",
            "bash",
            serde_json::json!({"command": "cargo test -p imp-core"}),
        ),
        make_tool_result("c1", "bash", "error[E0425]: cannot find value"),
    ];
    let prompt = build_summary_prompt_with_options(
        &messages,
        &SummaryPromptOptions {
            prompt: Some("CUSTOM HANDOFF TEMPLATE".into()),
            target_summary_tokens: Some(12_345),
        },
    );

    assert!(prompt.starts_with("CUSTOM HANDOFF TEMPLATE"));
    assert!(prompt.contains("TURNS TO SUMMARIZE"));
    assert!(prompt.contains("src/main.rs"));
    assert!(prompt.contains("cargo test -p imp-core"));
    assert!(prompt.contains("Target summary size: ~12345 tokens"));
}

#[test]
fn summary_prompt_options_fall_back_to_builtin_for_blank_custom_prompt() {
    let messages = vec![make_user("preserve this goal")];
    let prompt = build_summary_prompt_with_options(
        &messages,
        &SummaryPromptOptions {
            prompt: Some("   \n\t".into()),
            target_summary_tokens: Some(777),
        },
    );

    assert!(prompt.contains("Create a compact, high-signal handoff summary"));
    assert!(prompt.contains("Target about 777 tokens"));
    assert!(prompt.contains("preserve this goal"));
}

#[test]
fn auto_compaction_preserves_token_tail_and_drops_huge_tool_output() {
    let model = test_model();
    let huge_output = "x".repeat(80_000);
    let mut messages = Vec::new();
    for idx in 0..8 {
        let call_id = format!("call-{idx}");
        messages.push(make_user(&format!("inspect src/file_{idx}.rs")));
        messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/file_{idx}.rs")}),
        ));
        messages.push(make_tool_result(&call_id, "read", &huge_output));
    }
    messages.push(make_user("continue with the latest file"));

    let result = compact_messages_for_auto_compaction(&messages, &model, 4_000)
        .expect("tool-heavy context should compact");

    assert!(result.tokens_after < result.tokens_before);
    let compacted_json = serde_json::to_string(&result.messages).unwrap();
    assert!(compacted_json.contains("CONTEXT COMPACTION"));
    assert!(compacted_json.contains("src/file_7.rs"));
    assert!(compacted_json.contains("continue with the latest file"));
    assert!(!compacted_json.contains(&huge_output));
    assert!(compacted_json.contains("Tool result omitted"));
}

#[test]
fn context_compaction_groups_pull_in_prompting_user_messages() {
    let messages = vec![
        make_user("first prompt"),
        make_assistant_text("first answer"),
        make_user("second prompt"),
        make_assistant_tool_call("c1", "read", serde_json::json!({"path": "src/main.rs"})),
        make_tool_result("c1", "read", "fn main() {}"),
        make_assistant_text("done"),
    ];

    let groups = assistant_action_groups(&messages);
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].range, 0..3);
    assert_eq!(groups[1].range, 2..5);
    assert_eq!(groups[2].range, 5..6);
}

#[test]
fn context_compaction_prepare_keeps_recent_groups_verbatim() {
    let messages = vec![
        make_user("prompt 1"),
        make_assistant_text("answer 1"),
        make_user("prompt 2"),
        make_assistant_text("answer 2"),
        make_user("prompt 3"),
        make_assistant_text("answer 3"),
    ];

    let prepared = prepare_messages_for_compaction(&messages, 2);
    assert!(prepared.should_compact());
    assert_eq!(prepared.preserved_tail_start, 2);
    assert_eq!(prepared.summary_input.len(), 2);
    assert_eq!(prepared.preserved_tail.len(), 4);
    match &prepared.preserved_tail[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "prompt 2"),
            other => panic!("unexpected content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn context_compaction_prepare_shrinks_tool_heavy_prefix() {
    let large_output = "x".repeat(4000);
    let messages = vec![
        make_user("prompt 1"),
        make_assistant_tool_call("c1", "grep", serde_json::json!({"pattern": "foo"})),
        make_tool_result("c1", "grep", &large_output),
        make_user("prompt 2"),
        make_assistant_text("answer 2"),
    ];

    let original_bytes: usize = serde_json::to_string(&messages[..3]).unwrap().len();
    let prepared = prepare_messages_for_compaction(&messages, 1);
    let shrunk_bytes: usize = serde_json::to_string(&prepared.summary_input)
        .unwrap()
        .len();

    assert_eq!(prepared.shrunk_tool_results, 1);
    assert!(shrunk_bytes < original_bytes);
    let tool_result_text = match &prepared.summary_input[2] {
        Message::ToolResult(result) => match result.content.as_slice() {
            [ContentBlock::Text { text }] => text.clone(),
            other => panic!("unexpected tool result content: {other:?}"),
        },
        other => panic!("unexpected summary input message: {other:?}"),
    };
    assert!(tool_result_text.starts_with("[Output omitted"));
    assert!(tool_result_text.contains("grep"));
}

#[test]
fn context_compaction_prepare_sanitizes_unpaired_messages() {
    let messages = vec![
        make_user("prompt 1"),
        make_assistant_tool_call("c1", "grep", serde_json::json!({"pattern": "foo"})),
        make_user("prompt 2"),
        make_assistant_text("answer 2"),
    ];

    let prepared = prepare_messages_for_compaction(&messages, 1);
    assert_eq!(prepared.summary_input.len(), 1);
    match &prepared.summary_input[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "prompt 1"),
            other => panic!("unexpected content: {other:?}"),
        },
        other => panic!("unexpected summary input: {other:?}"),
    }
}

#[test]
fn context_compaction_prepare_noops_when_history_is_short() {
    let messages = vec![make_user("prompt"), make_assistant_text("answer")];
    let prepared = prepare_messages_for_compaction(&messages, 4);
    assert!(!prepared.should_compact());
    assert!(prepared.summary_input.is_empty());
    assert_eq!(prepared.preserved_tail.len(), 2);
}

// ── Executor tests ──────────────────────────────────────────────────

fn make_session_entry(id: &str, msg: Message) -> SessionEntry {
    SessionEntry::Message {
        id: id.into(),
        parent_id: None,
        message: msg,
    }
}

#[test]
fn compact_executor_passes_prompt_options_to_summarizer() {
    let mut mgr = SessionManager::in_memory();
    for idx in 0..6 {
        mgr.append(SessionEntry::Message {
            id: format!("u{idx}"),
            parent_id: None,
            message: make_user(&format!("preserve custom prompt path src/{idx}.rs")),
        })
        .unwrap();
        mgr.append(SessionEntry::Message {
            id: format!("a{idx}"),
            parent_id: None,
            message: make_assistant_text("ack"),
        })
        .unwrap();
    }

    let mut seen_prompt = String::new();
    let result = execute_manual_compaction_with_prompt_options(
        &mut mgr,
        2,
        &SummaryPromptOptions {
            prompt: Some("CUSTOM EXECUTOR PROMPT".into()),
            target_summary_tokens: Some(9_999),
        },
        |prompt| {
            seen_prompt = prompt.to_string();
            Ok(Some("executor summary".into()))
        },
    )
    .unwrap();

    assert!(result.is_some());
    assert!(seen_prompt.starts_with("CUSTOM EXECUTOR PROMPT"));
    assert!(seen_prompt.contains("Target summary size: ~9999 tokens"));
    assert!(seen_prompt.contains("src/0.rs"));
}

#[test]
fn compact_executor_persists_compaction_entry_and_changes_active_history() {
    let mut mgr = SessionManager::in_memory();
    mgr.append(make_session_entry("u1", make_user("first request")))
        .unwrap();
    mgr.append(make_session_entry(
        "a1",
        make_assistant_text("first answer"),
    ))
    .unwrap();
    mgr.append(make_session_entry("u2", make_user("second request")))
        .unwrap();
    mgr.append(make_session_entry(
        "a2",
        make_assistant_text("second answer"),
    ))
    .unwrap();
    mgr.append(make_session_entry("u3", make_user("third request")))
        .unwrap();
    mgr.append(make_session_entry(
        "a3",
        make_assistant_text("third answer"),
    ))
    .unwrap();

    let raw_before = mgr.get_messages().len();
    assert_eq!(raw_before, 6);

    let result = execute_manual_compaction(&mut mgr, 2, |_prompt| {
        Ok(Some("## Goal\nTest compaction".into()))
    })
    .unwrap();

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.summary.contains("CONTEXT COMPACTION"));
    assert!(result.summary.contains("Test compaction"));
    assert!(result.tokens_before > 0);
    assert!(result.tokens_after > 0);
    assert!(result.tokens_after <= result.tokens_before);

    // Raw messages are still preserved.
    let raw_after = mgr.get_messages().len();
    assert_eq!(raw_after, raw_before);

    // Active messages should now be: summary + preserved tail.
    let active = mgr.get_active_messages();
    assert!(active.len() < raw_before);
    // First active message should be the summary.
    match &active[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => {
                assert!(text.contains("CONTEXT COMPACTION"));
            }
            other => panic!("unexpected content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn compact_executor_returns_none_for_short_history() {
    let mut mgr = SessionManager::in_memory();
    mgr.append(make_session_entry("u1", make_user("only prompt")))
        .unwrap();
    mgr.append(make_session_entry("a1", make_assistant_text("only answer")))
        .unwrap();

    let result = execute_manual_compaction(&mut mgr, 4, |_| Ok(Some("summary".into()))).unwrap();
    assert!(result.is_none());
}

#[test]
fn compact_executor_uses_fallback_when_summarizer_returns_none() {
    let mut mgr = SessionManager::in_memory();
    for i in 0..6 {
        let uid = format!("u{i}");
        let aid = format!("a{i}");
        mgr.append(make_session_entry(&uid, make_user(&format!("prompt {i}"))))
            .unwrap();
        mgr.append(make_session_entry(
            &aid,
            make_assistant_text(&format!("answer {i}")),
        ))
        .unwrap();
    }

    let result = execute_manual_compaction(&mut mgr, 2, |_prompt| Ok(None)).unwrap();

    assert!(result.is_some());
    let result = result.unwrap();
    // The bounded fallback preserves high-value context when the LLM summarizer is skipped.
    assert!(result.summary.contains("Goal And User Instructions"));
    assert!(result.summary.contains("Recent Working Context"));
    assert!(result.summary.contains("prompt 0"));
}

#[test]
fn compact_executor_surfaces_summarizer_errors() {
    let mut mgr = SessionManager::in_memory();
    for i in 0..6 {
        let uid = format!("u{i}");
        let aid = format!("a{i}");
        mgr.append(make_session_entry(&uid, make_user(&format!("prompt {i}"))))
            .unwrap();
        mgr.append(make_session_entry(
            &aid,
            make_assistant_text(&format!("answer {i}")),
        ))
        .unwrap();
    }

    let result = execute_manual_compaction(&mut mgr, 2, |_prompt| {
        Err(crate::error::Error::Llm(imp_llm::Error::Provider(
            "summarizer failed".into(),
        )))
    });

    assert!(matches!(
        result,
        Err(crate::error::Error::Llm(imp_llm::Error::Provider(message)))
            if message == "summarizer failed"
    ));
    assert!(mgr.latest_compaction().is_none());
}
