use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream;
use futures_core::Stream;
use imp_llm::auth::AuthStore;
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::Provider;
use imp_llm::{AssistantMessage, Message, ThinkingLevel};

use super::*;
use crate::compaction::state::{ContinuationState, CONTINUATION_STATE_VERSION};
use crate::session::ActiveMessageSource;

struct FixtureProvider {
    events: Mutex<Option<Vec<StreamEvent>>>,
    options: Arc<Mutex<Option<RequestOptions>>>,
    prompt: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl Provider for FixtureProvider {
    fn stream(
        &self,
        _model: &Model,
        context: Context,
        options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        *self.options.lock().unwrap() = Some(options);
        *self.prompt.lock().unwrap() = Some(serde_json::to_string(&context.messages).unwrap());
        let events = self.events.lock().unwrap().take().unwrap();
        Box::pin(stream::iter(events.into_iter().map(Ok)))
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok("test-key".into())
    }

    fn id(&self) -> &str {
        "fixture"
    }

    fn models(&self) -> &[ModelMeta] {
        &[]
    }
}

fn response_stream(
    events: Vec<StreamEvent>,
) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
    Box::pin(stream::iter(events.into_iter().map(Ok)))
}

fn message_end(text: &str, stop_reason: StopReason) -> StreamEvent {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentBlock::Text { text: text.into() }]
    };
    StreamEvent::MessageEnd {
        message: AssistantMessage {
            content,
            usage: None,
            stop_reason,
            timestamp: 0,
        },
    }
}

fn valid_document() -> String {
    serde_json::json!({
        "version": 2,
        "summary": "validated Luna checkpoint",
        "fact_coverage": [{
            "fact_id": "entry:source-1",
            "summary_excerpt": "validated Luna checkpoint"
        }],
    })
    .to_string()
}

fn active(id: &str, text: &str) -> ActiveSessionMessage {
    ActiveSessionMessage {
        source: ActiveMessageSource::Message {
            entry_id: id.into(),
        },
        message: Message::user(text),
    }
}

fn config() -> SummarizerConfig {
    SummarizerConfig {
        model: "gpt-5.6-luna".into(),
        reserve_tokens: 32_000,
        target_summary_tokens: 8_000,
        thinking: ThinkingLevel::XHigh,
        checkpoint_interval_tokens: 1,
        system_prompt: Some("CUSTOM COMPACTION SYSTEM".into()),
    }
}

fn model(
    events: Vec<StreamEvent>,
) -> (
    Model,
    Arc<Mutex<Option<RequestOptions>>>,
    Arc<Mutex<Option<String>>>,
) {
    let options = Arc::new(Mutex::new(None));
    let prompt = Arc::new(Mutex::new(None));
    let provider = FixtureProvider {
        events: Mutex::new(Some(events)),
        options: Arc::clone(&options),
        prompt: Arc::clone(&prompt),
    };
    let model = Model {
        meta: ModelMeta {
            id: "gpt-5.6-luna".into(),
            provider: "openai".into(),
            name: "Luna".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities::default(),
        },
        provider: Arc::new(provider),
    };
    (model, options, prompt)
}

fn request(store: CheckpointStore, model: Model) -> CheckpointRequest {
    CheckpointRequest {
        active: vec![active("source-1", &"important objective ".repeat(100))],
        previous: None,
        store,
        model,
        api_key: "test-key".into(),
        config: config(),
        authoritative_state: None,
        generation_mode: CheckpointGenerationMode::WhenDue,
    }
}

#[tokio::test]
async fn coordinator_uses_generation_reserve_and_publishes_valid_document() {
    let temp = tempfile::tempdir().unwrap();
    let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
    let document = valid_document();
    let (model, options, prompt) = model(vec![
        StreamEvent::TextDelta {
            text: document.clone(),
        },
        message_end(&document, StopReason::EndTurn),
    ]);

    generate_checkpoint(request(store.clone(), model))
        .await
        .unwrap();

    let checkpoint = store.load().unwrap().unwrap();
    assert_eq!(checkpoint.continuation.version, CONTINUATION_STATE_VERSION);
    assert_eq!(checkpoint.summary, "validated Luna checkpoint");
    let options = options.lock().unwrap();
    let options = options.as_ref().unwrap();
    assert_eq!(options.thinking_level, ThinkingLevel::XHigh);
    assert_eq!(options.max_tokens, Some(32_000));
    assert_eq!(options.system_prompt, "CUSTOM COMPACTION SYSTEM");
    let prompt = prompt.lock().unwrap();
    assert!(prompt.as_ref().unwrap().contains("summary_target_tokens"));
    assert!(prompt.as_ref().unwrap().contains("8000"));
}

#[tokio::test]
async fn forced_checkpoint_generation_ignores_rolling_interval() {
    let temp = tempfile::tempdir().unwrap();
    let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
    let document = valid_document();
    let (model, _, _) = model(vec![
        StreamEvent::TextDelta {
            text: document.clone(),
        },
        message_end(&document, StopReason::EndTurn),
    ]);
    let mut request = request(store.clone(), model);
    request.config.checkpoint_interval_tokens = 1_000_000;
    request.generation_mode = CheckpointGenerationMode::Force;

    generate_checkpoint(request).await.unwrap();

    assert_eq!(
        store.load().unwrap().unwrap().summary,
        "validated Luna checkpoint"
    );
}

#[tokio::test]
async fn empty_completion_is_explicit_and_preserves_previous_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
    let previous = CompactionCheckpoint {
        version: CHECKPOINT_VERSION,
        id: "previous".into(),
        previous_checkpoint_id: None,
        covered_entry_ids: vec!["source-1".into()],
        source_fingerprint: "previous-source".into(),
        continuation: ContinuationState {
            version: CONTINUATION_STATE_VERSION,
            facts: Vec::new(),
        },
        summary: "previous valid checkpoint".into(),
        model_id: "gpt-5.6-luna".into(),
        provider_id: "openai".into(),
        thinking: "xhigh".into(),
        source_tokens: 1,
    };
    store.publish(&previous).unwrap();
    let (model, _, _) = model(vec![message_end("", StopReason::EndTurn)]);
    let mut request = request(store.clone(), model);
    request
        .active
        .push(active("source-2", &"new context ".repeat(100)));
    request.previous = Some(previous.clone());

    let error = generate_checkpoint(request).await.unwrap_err();

    assert!(error.to_string().contains("returned no JSON text"));
    assert_eq!(store.load().unwrap(), Some(previous));
}

#[tokio::test]
async fn max_tokens_is_reported_before_json_parsing() {
    let mut events = response_stream(vec![message_end("{\"version\":2", StopReason::MaxTokens)]);

    let error = collect_response(&mut events).await.unwrap_err();

    assert!(error
        .to_string()
        .contains("exhausted its generation token allowance"));
    assert!(!error.to_string().contains("JSON error"));
}

#[tokio::test]
async fn malformed_json_and_stream_failures_are_distinct() {
    let error = parse_document("{\"version\":2", 32_000).unwrap_err();
    assert!(error.to_string().contains("returned truncated JSON"));
    assert!(error.to_string().contains("32000 tokens"));

    let mut events = response_stream(vec![StreamEvent::Error {
        error: "provider disconnected".into(),
    }]);
    let error = collect_response(&mut events).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("model stream failed: provider disconnected"));

    let mut events = response_stream(vec![StreamEvent::TextDelta {
        text: valid_document(),
    }]);
    let error = collect_response(&mut events).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("stream ended before a completion event"));
}
