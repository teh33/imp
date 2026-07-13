use imp_llm::Message;

use crate::compaction::record::{CompactionRecord, CompactionTrigger, COMPACTION_RECORD_VERSION};
use crate::context::estimate_tokens;
use crate::error::{Error, Result};
use crate::session::{ActiveMessageSource, ActiveSessionMessage, SessionEntry, SessionManager};

pub mod checkpoint;
pub mod coordinator;
pub mod record;
pub mod state;

pub const COMPACTION_SUMMARY_PREFIX: &str = "[CONTEXT COMPACTION] Earlier turns were compacted. \
Use the validated checkpoint below plus the preserved recent messages to continue. \
Raw source history remains available in the durable session:\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_id: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub compaction_entry_id: String,
}

pub fn activate_checkpoint_for_usage(
    session: &mut SessionManager,
    used_tokens: u32,
    input_limit: u32,
    trigger_ratio: f64,
) -> Result<Option<CompactionResult>> {
    let threshold = (f64::from(input_limit.max(1)) * trigger_ratio.clamp(0.0, 1.0)) as u32;
    if used_tokens < threshold {
        return Ok(None);
    }
    let path = session.path().ok_or_else(|| {
        Error::Config("automatic compaction requires a durable session".to_string())
    })?;
    let store = checkpoint::CheckpointStore::for_session(path);
    let checkpoint = store
        .load()?
        .ok_or_else(|| Error::Config("no validated compaction checkpoint is ready".to_string()))?;
    activate_checkpoint_with_trigger(session, &checkpoint, CompactionTrigger::Automatic).map(Some)
}

pub fn activate_checkpoint(
    session: &mut SessionManager,
    checkpoint: &checkpoint::CompactionCheckpoint,
) -> Result<CompactionResult> {
    activate_checkpoint_with_trigger(session, checkpoint, CompactionTrigger::Manual)
}

fn activate_checkpoint_with_trigger(
    session: &mut SessionManager,
    checkpoint: &checkpoint::CompactionCheckpoint,
    trigger: CompactionTrigger,
) -> Result<CompactionResult> {
    let active = session.get_active_message_entries();
    let source = checkpoint::checkpoint_source(&active, Some(checkpoint))?;
    let preserved = &active[source.uncovered_start..];
    let first_kept_id = preserved.iter().find_map(raw_entry_id).unwrap_or_default();
    let preserved_entry_ids = preserved
        .iter()
        .filter_map(raw_entry_id)
        .collect::<Vec<_>>();
    let messages = active_messages(&active);
    let preserved_messages = active_messages(preserved);
    let summary = checkpoint_summary(checkpoint);
    let tokens_before = estimate_message_tokens(&messages);
    let tokens_after = estimate_tokens(&summary) + estimate_message_tokens(&preserved_messages);
    if tokens_after >= tokens_before {
        return Err(Error::Config(
            "validated checkpoint would not reduce active context".to_string(),
        ));
    }

    let entry_id = uuid::Uuid::new_v4().to_string();
    session.append(SessionEntry::CompactionV2 {
        id: entry_id.clone(),
        parent_id: None,
        record: CompactionRecord {
            version: COMPACTION_RECORD_VERSION,
            trigger,
            summary: summary.clone(),
            source_entry_ids: source.entry_ids,
            preserved_entry_ids,
            first_kept_id: first_kept_id.clone(),
            continuation: checkpoint.continuation.clone(),
            model_id: checkpoint.model_id.clone(),
            provider_id: checkpoint.provider_id.clone(),
            tokens_before,
            tokens_after,
        },
    })?;
    Ok(CompactionResult {
        summary,
        first_kept_id,
        tokens_before,
        tokens_after,
        compaction_entry_id: entry_id,
    })
}

fn checkpoint_summary(checkpoint: &checkpoint::CompactionCheckpoint) -> String {
    let evidence = checkpoint
        .continuation
        .facts
        .iter()
        .filter(|fact| fact.required)
        .map(|fact| {
            format!(
                "- [{} from session entry {}] {}",
                fact.id, fact.source_entry_id, fact.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if evidence.is_empty() {
        format!("{COMPACTION_SUMMARY_PREFIX}{}", checkpoint.summary)
    } else {
        format!(
            "{COMPACTION_SUMMARY_PREFIX}{}\n\n## Required source-backed facts\n{evidence}",
            checkpoint.summary
        )
    }
}

fn raw_entry_id(entry: &ActiveSessionMessage) -> Option<String> {
    match &entry.source {
        ActiveMessageSource::Message { entry_id } => Some(entry_id.clone()),
        ActiveMessageSource::Compaction { .. } => None,
    }
}

fn active_messages(entries: &[ActiveSessionMessage]) -> Vec<Message> {
    entries.iter().map(|entry| entry.message.clone()).collect()
}

fn estimate_message_tokens(messages: &[Message]) -> u32 {
    messages
        .iter()
        .map(|message| estimate_tokens(&serde_json::to_string(message).unwrap_or_default()))
        .sum()
}

#[cfg(test)]
#[path = "compaction/tests.rs"]
mod tests;
