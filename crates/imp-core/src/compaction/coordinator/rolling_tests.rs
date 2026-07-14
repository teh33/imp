use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream;
use futures_core::Stream;
use imp_llm::auth::AuthStore;
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::{Context, Provider, RequestOptions};
use imp_llm::{AssistantMessage, ContentBlock, Message, StopReason, StreamEvent, ThinkingLevel};

use super::*;
use crate::compaction::state::{ContinuationState, CONTINUATION_STATE_VERSION};
use crate::session::ActiveMessageSource;

struct SequencedProvider {
    responses: Mutex<VecDeque<Vec<StreamEvent>>>,
    prompt_sizes: Arc<Mutex<Vec<usize>>>,
}

#[async_trait]
impl Provider for SequencedProvider {
    fn stream(
        &self,
        _model: &Model,
        context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        let prompt = serde_json::to_string(&context.messages).unwrap();
        self.prompt_sizes.lock().unwrap().push(prompt.len());
        let events = self.responses.lock().unwrap().pop_front().unwrap();
        Box::pin(stream::iter(events.into_iter().map(Ok)))
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok("test-key".into())
    }

    fn id(&self) -> &str {
        "sequenced-fixture"
    }

    fn models(&self) -> &[ModelMeta] {
        &[]
    }
}

fn active(id: &str, text: &str) -> ActiveSessionMessage {
    ActiveSessionMessage {
        source: ActiveMessageSource::Message {
            entry_id: id.into(),
        },
        message: Message::user(text),
    }
}

fn previous_checkpoint() -> CompactionCheckpoint {
    CompactionCheckpoint {
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
        model_id: "fixture".into(),
        provider_id: "fixture".into(),
        thinking: "off".into(),
        source_tokens: 1,
    }
}

fn document_end() -> Vec<StreamEvent> {
    let text = serde_json::json!({
        "version": 2,
        "summary": "first rolling stage",
        "fact_coverage": [],
    })
    .to_string();
    vec![StreamEvent::MessageEnd {
        message: AssistantMessage {
            content: vec![ContentBlock::Text { text }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 0,
        },
    }]
}

#[tokio::test]
async fn later_chunk_failure_preserves_previous_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
    let previous = previous_checkpoint();
    store.publish(&previous).unwrap();
    let prompt_sizes = Arc::new(Mutex::new(Vec::new()));
    let provider = SequencedProvider {
        responses: Mutex::new(VecDeque::from([
            document_end(),
            vec![StreamEvent::Error {
                error: "second chunk failed".into(),
            }],
        ])),
        prompt_sizes: Arc::clone(&prompt_sizes),
    };
    let model = Model {
        meta: ModelMeta {
            id: "fixture".into(),
            provider: "fixture".into(),
            name: "Fixture".into(),
            context_window: 20_000,
            max_output_tokens: 512,
            pricing: ModelPricing::default(),
            capabilities: Capabilities::default(),
        },
        provider: Arc::new(provider),
    };
    let request = CheckpointRequest {
        active: vec![
            active("source-1", "covered"),
            active("source-2", &"new context ".repeat(20_000)),
        ],
        previous: Some(previous.clone()),
        store: store.clone(),
        model,
        api_key: "test-key".into(),
        config: SummarizerConfig {
            model: "fixture".into(),
            reserve_tokens: 512,
            target_summary_tokens: 256,
            thinking: ThinkingLevel::Off,
            checkpoint_interval_tokens: 1,
            system_prompt: Some("summarize safely".into()),
        },
        authoritative_state: None,
    };

    let error = generate_checkpoint(request).await.unwrap_err();

    assert!(error.to_string().contains("second chunk failed"));
    assert_eq!(prompt_sizes.lock().unwrap().len(), 2);
    assert_eq!(store.load().unwrap(), Some(previous));
}
