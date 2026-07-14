use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use imp_llm::auth::ApiKey;
use imp_llm::provider::{CacheOptions, Context, RequestOptions};
use imp_llm::{ContentBlock, Model, StopReason, StreamEvent};

use super::checkpoint::{
    checkpoint_source_for_model, CheckpointStore, CompactionCheckpoint, CHECKPOINT_VERSION,
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
    let source = checkpoint_source_for_model(
        &request.active,
        request.previous.as_ref(),
        &request.model.meta,
    )?;
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
        request.config.target_summary_tokens,
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
    summary_target_tokens: u32,
) -> Result<String> {
    let messages = uncovered
        .iter()
        .map(|entry| &entry.message)
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "schema": {
            "version": 2,
            "summary": "string",
            "fact_coverage": [{
                "fact_id": "required fact ID",
                "summary_excerpt": "exact non-empty substring from summary representing that fact"
            }],
        },
        "summary_target_tokens": summary_target_tokens,
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
    let generation_tokens = generation_token_limit(request)?;
    let options = RequestOptions {
        thinking_level: request.config.thinking,
        max_tokens: Some(generation_tokens),
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
    let content = collect_response(&mut stream).await?;
    parse_document(&content, generation_tokens)
}

async fn collect_response(
    stream: &mut Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>,
) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(180), async {
        let mut text = String::new();
        let mut completed = false;
        while let Some(event) = stream.next().await {
            match event.map_err(Error::Llm)? {
                StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
                StreamEvent::MessageEnd { message } => {
                    if text.is_empty() {
                        text = message_text(&message.content);
                    }
                    validate_stop_reason(&message.stop_reason)?;
                    completed = true;
                }
                StreamEvent::Error { error } => {
                    return Err(Error::Config(format!(
                        "compaction checkpoint model stream failed: {error}"
                    )));
                }
                _ => {}
            }
        }
        if !completed {
            return Err(Error::Config(
                "compaction checkpoint model stream ended before a completion event".to_string(),
            ));
        }
        if text.trim().is_empty() {
            return Err(Error::Config(
                "compaction checkpoint model returned no JSON text".to_string(),
            ));
        }
        Ok(text)
    })
    .await
    .map_err(|_| Error::Config("compaction checkpoint model timed out".to_string()))?
}

fn message_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn validate_stop_reason(reason: &StopReason) -> Result<()> {
    match reason {
        StopReason::EndTurn => Ok(()),
        StopReason::MaxTokens => Err(Error::Config(
            "compaction checkpoint model exhausted its generation token allowance before producing a complete JSON document"
                .to_string(),
        )),
        StopReason::Error(error) => Err(Error::Config(format!(
            "compaction checkpoint model failed: {error}"
        ))),
        StopReason::ToolUse => Err(Error::Config(
            "compaction checkpoint model unexpectedly requested a tool".to_string(),
        )),
    }
}

fn parse_document(content: &str, generation_tokens: u32) -> Result<CompactionDocument> {
    serde_json::from_str(content.trim()).map_err(|error| {
        let detail = if error.is_eof() {
            format!(
                "returned truncated JSON after {} bytes; generation allowance was {generation_tokens} tokens",
                content.len()
            )
        } else {
            format!("returned invalid JSON after {} bytes: {error}", content.len())
        };
        Error::Config(format!("compaction checkpoint model {detail}"))
    })
}

fn generation_token_limit(request: &CheckpointRequest) -> Result<u32> {
    let configured = request
        .config
        .reserve_tokens
        .max(request.config.target_summary_tokens);
    let limit = configured.min(request.model.meta.max_output_tokens);
    if limit == 0 {
        return Err(Error::Config(
            "compaction checkpoint generation token allowance is zero".to_string(),
        ));
    }
    Ok(limit)
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
#[path = "coordinator_tests.rs"]
mod tests;
