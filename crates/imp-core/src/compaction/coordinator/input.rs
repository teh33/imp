use imp_llm::ModelMeta;
use serde_json::json;

use super::super::checkpoint::CompactionCheckpoint;
use super::super::state::ContinuationState;
use crate::context::{context_budget_for_meta, estimate_text_tokens_for_model};
use crate::error::{Error, Result};
use crate::session::ActiveSessionMessage;

pub(super) const MAX_PROVIDER_CONTENT_BYTES: usize = 9 * 1024 * 1024;
const TOKEN_HEADROOM_PERCENT: u32 = 90;

pub(super) struct PromptChunk {
    pub prompt: String,
    pub end: usize,
}

pub(super) fn serialize_source(entries: &[ActiveSessionMessage]) -> Result<String> {
    let messages = entries
        .iter()
        .map(|entry| &entry.message)
        .collect::<Vec<_>>();
    serde_json::to_string(&messages).map_err(Into::into)
}

pub(super) fn next_prompt(
    source: &str,
    start: usize,
    prior_summary: Option<&str>,
    previous: Option<&CompactionCheckpoint>,
    continuation: &ContinuationState,
    summary_target_tokens: u32,
    generation_tokens: u32,
    system_prompt: &str,
    model: &ModelMeta,
) -> Result<PromptChunk> {
    if start >= source.len() {
        return Err(Error::Config(
            "compaction checkpoint source is already exhausted".to_string(),
        ));
    }
    validate_system_prompt(system_prompt)?;
    let token_limit = prompt_token_limit(model, generation_tokens, system_prompt)?;
    let empty = checkpoint_prompt(
        "",
        start,
        start,
        source.len(),
        prior_summary,
        previous,
        continuation,
        summary_target_tokens,
    )?;
    validate_envelope(&empty, token_limit, model)?;

    let mut take = source.len() - start;
    take = take.min(MAX_PROVIDER_CONTENT_BYTES.saturating_sub(empty.len()));
    loop {
        let end = chunk_end(source, start, take)?;
        let prompt = checkpoint_prompt(
            &source[start..end],
            start,
            end,
            source.len(),
            prior_summary,
            previous,
            continuation,
            summary_target_tokens,
        )?;
        let tokens = estimate_text_tokens_for_model(&prompt, model);
        if prompt.len() <= MAX_PROVIDER_CONTENT_BYTES && tokens <= token_limit {
            return Ok(PromptChunk { prompt, end });
        }
        if take == 1 {
            return Err(Error::Config(
                "compaction checkpoint input budget cannot fit one escaped source character"
                    .to_string(),
            ));
        }
        take = reduced_take(take, prompt.len(), tokens, token_limit);
    }
}

#[allow(clippy::too_many_arguments)]
fn checkpoint_prompt(
    source_chunk: &str,
    start: usize,
    end: usize,
    total: usize,
    prior_summary: Option<&str>,
    previous: Option<&CompactionCheckpoint>,
    continuation: &ContinuationState,
    summary_target_tokens: u32,
) -> Result<String> {
    let value = json!({
        "schema": {
            "version": 2,
            "summary": "string",
            "fact_coverage": [{
                "fact_id": "required fact ID",
                "summary_excerpt": "exact non-empty substring from summary representing that fact"
            }],
        },
        "instructions": "Update the rolling summary with this source chunk. Preserve all prior facts, add every relevant new fact, and return the complete requested JSON document.",
        "summary_target_tokens": summary_target_tokens,
        "previous_checkpoint_summary": previous.map(|value| value.summary.as_str()),
        "prior_rolling_summary": prior_summary,
        "authoritative_continuation_state": continuation,
        "source_format": "Ordered JSON session-message array split losslessly by UTF-8 byte range; a chunk may begin or end inside a JSON value.",
        "source_range": {"start_byte": start, "end_byte": end, "total_bytes": total},
        "is_final_chunk": end == total,
        "source_chunk": source_chunk,
    });
    serde_json::to_string(&value).map_err(Into::into)
}

fn prompt_token_limit(
    model: &ModelMeta,
    generation_tokens: u32,
    system_prompt: &str,
) -> Result<u32> {
    let budget = context_budget_for_meta(model);
    let available = budget
        .input_limit
        .saturating_sub(generation_tokens)
        .saturating_mul(TOKEN_HEADROOM_PERCENT)
        / 100;
    let system_tokens = estimate_text_tokens_for_model(system_prompt, model);
    let limit = available.saturating_sub(system_tokens);
    if limit == 0 {
        return Err(Error::Config(
            "compaction model has no input capacity after system prompt and generation reserve"
                .to_string(),
        ));
    }
    Ok(limit)
}

fn validate_system_prompt(prompt: &str) -> Result<()> {
    if prompt.len() > MAX_PROVIDER_CONTENT_BYTES {
        return Err(Error::Config(format!(
            "compaction system prompt is too large: {} bytes exceeds the {} byte safety limit",
            prompt.len(),
            MAX_PROVIDER_CONTENT_BYTES
        )));
    }
    Ok(())
}

fn validate_envelope(prompt: &str, token_limit: u32, model: &ModelMeta) -> Result<()> {
    let tokens = estimate_text_tokens_for_model(prompt, model);
    if prompt.len() > MAX_PROVIDER_CONTENT_BYTES || tokens > token_limit {
        return Err(Error::Config(format!(
            "compaction checkpoint metadata exceeds the safe provider input budget ({} bytes, {tokens} estimated tokens)",
            prompt.len()
        )));
    }
    Ok(())
}

fn chunk_end(source: &str, start: usize, take: usize) -> Result<usize> {
    let mut end = start.saturating_add(take).min(source.len());
    while end > start && !source.is_char_boundary(end) {
        end -= 1;
    }
    if end == start {
        return Err(Error::Config(
            "compaction checkpoint input budget cannot fit one UTF-8 character".to_string(),
        ));
    }
    Ok(end)
}

fn reduced_take(take: usize, prompt_bytes: usize, tokens: u32, token_limit: u32) -> usize {
    let byte_ratio = MAX_PROVIDER_CONTENT_BYTES.saturating_mul(take) / prompt_bytes.max(1);
    let token_ratio = (u64::from(token_limit) * take as u64 / u64::from(tokens.max(1))) as usize;
    let next = byte_ratio.min(token_ratio).saturating_mul(95) / 100;
    next.min(take.saturating_sub(1)).max(1)
}

#[cfg(test)]
mod tests {
    use imp_llm::model::{Capabilities, ModelPricing};

    use super::*;
    use crate::compaction::state::CONTINUATION_STATE_VERSION;

    fn large_context_model() -> ModelMeta {
        ModelMeta {
            id: "fixture-large-context".into(),
            provider: "fixture".into(),
            name: "Fixture".into(),
            context_window: 50_000_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities::default(),
        }
    }

    #[test]
    fn oversized_escaped_utf8_source_is_split_losslessly() {
        let pattern = "quoted: \\\"path\\\\value\\\"; unicode: 🙂\n";
        let source = pattern.repeat(350_000);
        assert!(source.len() > 10_485_760);
        let state = ContinuationState {
            version: CONTINUATION_STATE_VERSION,
            facts: Vec::new(),
        };
        let model = large_context_model();
        let mut offset = 0;
        let mut reconstructed = String::new();
        let mut requests = 0;

        while offset < source.len() {
            let chunk = next_prompt(
                &source,
                offset,
                Some("rolling summary"),
                None,
                &state,
                8_000,
                32_000,
                "summarize safely",
                &model,
            )
            .unwrap();
            assert!(chunk.prompt.len() <= MAX_PROVIDER_CONTENT_BYTES);
            let prompt: serde_json::Value = serde_json::from_str(&chunk.prompt).unwrap();
            reconstructed.push_str(prompt["source_chunk"].as_str().unwrap());
            assert_eq!(prompt["source_range"]["start_byte"], offset);
            assert_eq!(prompt["source_range"]["end_byte"], chunk.end);
            assert!(source.is_char_boundary(chunk.end));
            offset = chunk.end;
            requests += 1;
        }

        assert!(requests >= 2);
        assert_eq!(reconstructed, source);
    }
}
