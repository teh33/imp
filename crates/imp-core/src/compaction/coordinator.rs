use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use imp_llm::auth::ApiKey;
use imp_llm::provider::{CacheOptions, Context, RequestOptions};
use imp_llm::{ContentBlock, Model, StreamEvent};

use super::checkpoint::{
    checkpoint_source, CheckpointStore, CompactionCheckpoint, CHECKPOINT_VERSION,
};
use super::prompt::DEFAULT_SYSTEM_PROMPT;
use super::record::{validate_document, CompactionDocument};
use super::state::{extract_continuation_state, merge_continuation_state};
use crate::config::SummarizerConfig;
use crate::error::{Error, Result};
use crate::session::ActiveSessionMessage;

pub struct CheckpointRequest {
    pub active: Vec<ActiveSessionMessage>,
    pub previous: Option<CompactionCheckpoint>,
    pub store: CheckpointStore,
    pub model: Model,
    pub api_key: ApiKey,
    pub config: SummarizerConfig,
    pub authoritative_state: Option<String>,
}

pub async fn generate_checkpoint(request: CheckpointRequest) -> Result<()> {
    let source = checkpoint_source(&request.active, request.previous.as_ref())?;
    if !source.is_due(request.config.checkpoint_interval_tokens) {
        return Ok(());
    }
    let delta = extract_continuation_state(&request.active[source.uncovered_start..]);
    let inherited = request
        .previous
        .as_ref()
        .map(|checkpoint| &checkpoint.continuation)
        .or(source.inherited_continuation.as_ref());
    let continuation = merge_continuation_state(inherited, delta);
    let continuation =
        merge_authoritative_projection(continuation, request.authoritative_state.as_deref());
    let prompt = checkpoint_prompt(
        &request.active[source.uncovered_start..],
        request.previous.as_ref(),
        &continuation,
    )?;
    let document = request_document(&request, prompt).await?;
    validate_document(&continuation, &document).map_err(Error::Config)?;
    let checkpoint = CompactionCheckpoint {
        version: CHECKPOINT_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        previous_checkpoint_id: request.previous.as_ref().map(|value| value.id.clone()),
        covered_entry_ids: source.entry_ids,
        source_fingerprint: source.source_fingerprint,
        continuation,
        summary: document.summary,
        model_id: request.model.meta.id.clone(),
        provider_id: request.model.meta.provider.clone(),
        thinking: thinking_name(request.config.thinking).to_string(),
        source_tokens: source.uncovered_tokens,
    };
    request.store.publish(&checkpoint)
}

fn merge_authoritative_projection(
    mut continuation: super::state::ContinuationState,
    projection: Option<&str>,
) -> super::state::ContinuationState {
    let Some(projection) = projection.filter(|value| !value.trim().is_empty()) else {
        return continuation;
    };
    continuation
        .facts
        .retain(|fact| fact.id != "runtime:task-state");
    continuation.facts.push(super::state::StateFact {
        id: "runtime:task-state".to_string(),
        kind: super::state::FactKind::Obligation,
        text: projection.to_string(),
        source_entry_id: "runtime:task-state".to_string(),
        required: true,
    });
    continuation
}

fn checkpoint_prompt(
    uncovered: &[ActiveSessionMessage],
    previous: Option<&CompactionCheckpoint>,
    continuation: &super::state::ContinuationState,
) -> Result<String> {
    let messages = uncovered
        .iter()
        .map(|entry| &entry.message)
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "schema": {
            "version": 2,
            "summary": "string",
            "acknowledged_fact_ids": continuation.required_fact_ids(),
        },
        "previous_checkpoint": previous,
        "authoritative_continuation_state": continuation,
        "uncovered_messages": messages,
    });
    serde_json::to_string(&value).map_err(Into::into)
}

async fn request_document(
    request: &CheckpointRequest,
    prompt: String,
) -> Result<CompactionDocument> {
    let context = Context {
        messages: vec![imp_llm::Message::user(prompt)],
        session_id: None,
        thread_id: None,
    };
    let options = RequestOptions {
        thinking_level: request.config.thinking,
        max_tokens: Some(request.config.target_summary_tokens),
        temperature: Some(0.2),
        system_prompt: request
            .config
            .system_prompt
            .clone()
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        tools: Vec::new(),
        cache_options: CacheOptions::default(),
        effort: None,
    };
    let model = clone_model(&request.model);
    let api_key = request.api_key.clone();
    let mut stream = model.provider.stream(&model, context, options, &api_key);
    let content = tokio::time::timeout(Duration::from_secs(180), async move {
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.map_err(Error::Llm)? {
                StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
                StreamEvent::MessageEnd { message } if text.is_empty() => {
                    text = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                }
                _ => {}
            }
        }
        Ok::<_, Error>(text)
    })
    .await
    .map_err(|_| Error::Config("compaction checkpoint model timed out".to_string()))??;
    serde_json::from_str(content.trim()).map_err(Into::into)
}

fn thinking_name(level: imp_llm::ThinkingLevel) -> &'static str {
    match level {
        imp_llm::ThinkingLevel::Off => "off",
        imp_llm::ThinkingLevel::Minimal => "minimal",
        imp_llm::ThinkingLevel::Low => "low",
        imp_llm::ThinkingLevel::Medium => "medium",
        imp_llm::ThinkingLevel::High => "high",
        imp_llm::ThinkingLevel::XHigh => "xhigh",
    }
}

fn clone_model(model: &Model) -> Model {
    Model {
        meta: model.meta.clone(),
        provider: Arc::clone(&model.provider),
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::stream;
    use futures_core::Stream;
    use imp_llm::auth::AuthStore;
    use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
    use imp_llm::provider::Provider;
    use imp_llm::{Message, ThinkingLevel};

    use super::*;
    use crate::compaction::state::CONTINUATION_STATE_VERSION;
    use crate::session::ActiveMessageSource;

    struct CaptureProvider {
        options: Arc<Mutex<Option<RequestOptions>>>,
    }

    #[async_trait]
    impl Provider for CaptureProvider {
        fn stream(
            &self,
            _model: &Model,
            context: Context,
            options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            *self.options.lock().unwrap() = Some(options);
            let prompt = serde_json::to_value(context.messages).unwrap().to_string();
            assert!(prompt.contains("entry:source-1"));
            let document = serde_json::json!({
                "version": 2,
                "summary": "validated Luna checkpoint",
                "acknowledged_fact_ids": ["entry:source-1"],
            });
            Box::pin(stream::iter([Ok(StreamEvent::TextDelta {
                text: document.to_string(),
            })]))
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("test-key".into())
        }

        fn id(&self) -> &str {
            "capture"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    #[tokio::test]
    async fn coordinator_uses_configured_xhigh_prompt_and_publishes_valid_document() {
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
        let options = Arc::new(Mutex::new(None));
        let provider = CaptureProvider {
            options: Arc::clone(&options),
        };
        let active = vec![ActiveSessionMessage {
            source: ActiveMessageSource::Message {
                entry_id: "source-1".into(),
            },
            message: Message::user(&"important objective ".repeat(100)),
        }];
        let config = SummarizerConfig {
            model: "gpt-5.6-luna".into(),
            reserve_tokens: 32_000,
            target_summary_tokens: 8_000,
            thinking: ThinkingLevel::XHigh,
            checkpoint_interval_tokens: 1,
            system_prompt: Some("CUSTOM COMPACTION SYSTEM".into()),
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

        generate_checkpoint(CheckpointRequest {
            active,
            previous: None,
            store: store.clone(),
            model,
            api_key: "test-key".into(),
            config,
            authoritative_state: None,
        })
        .await
        .unwrap();

        let checkpoint = store.load().unwrap().unwrap();
        assert_eq!(checkpoint.version, CHECKPOINT_VERSION);
        assert_eq!(checkpoint.continuation.version, CONTINUATION_STATE_VERSION);
        assert_eq!(checkpoint.summary, "validated Luna checkpoint");
        assert_eq!(checkpoint.thinking, "xhigh");
        let captured = options.lock().unwrap();
        let captured = captured.as_ref().unwrap();
        assert_eq!(captured.thinking_level, ThinkingLevel::XHigh);
        assert_eq!(captured.system_prompt, "CUSTOM COMPACTION SYSTEM");
    }
}
