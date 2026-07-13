use imp_llm::{ContentBlock, Message};
use serde::{Deserialize, Serialize};

use crate::session::{ActiveMessageSource, ActiveSessionMessage};

pub const CONTINUATION_STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactKind {
    Objective,
    Constraint,
    Decision,
    Effect,
    Verification,
    Obligation,
    Blocker,
    Artifact,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateFact {
    pub id: String,
    pub kind: FactKind,
    pub text: String,
    pub source_entry_id: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationState {
    pub version: u32,
    pub facts: Vec<StateFact>,
}

impl ContinuationState {
    pub fn required_fact_ids(&self) -> Vec<&str> {
        self.facts
            .iter()
            .filter(|fact| fact.required)
            .map(|fact| fact.id.as_str())
            .collect()
    }
}

pub fn extract_continuation_state(entries: &[ActiveSessionMessage]) -> ContinuationState {
    let facts = entries.iter().filter_map(fact_from_entry).collect();
    ContinuationState {
        version: CONTINUATION_STATE_VERSION,
        facts,
    }
}

pub fn merge_continuation_state(
    previous: Option<&ContinuationState>,
    delta: ContinuationState,
) -> ContinuationState {
    let mut facts = previous
        .map(|state| state.facts.clone())
        .unwrap_or_default();
    for fact in delta.facts {
        if let Some(existing) = facts.iter_mut().find(|existing| existing.id == fact.id) {
            *existing = fact;
        } else {
            facts.push(fact);
        }
    }
    ContinuationState {
        version: CONTINUATION_STATE_VERSION,
        facts,
    }
}

fn fact_from_entry(entry: &ActiveSessionMessage) -> Option<StateFact> {
    let source_entry_id = match &entry.source {
        ActiveMessageSource::Compaction { entry_id, .. }
        | ActiveMessageSource::Message { entry_id } => entry_id.clone(),
    };
    let text = message_text(&entry.message);
    let (kind, required) = match &entry.message {
        Message::User(_) => (FactKind::Objective, true),
        Message::Assistant(_) => (FactKind::Context, true),
        Message::ToolResult(result) if result.is_error => (FactKind::Blocker, true),
        Message::ToolResult(_) => (FactKind::Verification, true),
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(StateFact {
        id: format!("entry:{source_entry_id}"),
        kind,
        text: truncate(text, 4_000),
        source_entry_id,
        required,
    })
}

fn message_text(message: &Message) -> String {
    match message {
        Message::User(message) => content_blocks(&message.content),
        Message::Assistant(message) => content_blocks(&message.content),
        Message::ToolResult(result) => {
            let details = concise_details(&result.details);
            let content = content_blocks(&result.content);
            match (details.is_empty(), content.is_empty()) {
                (false, false) => format!("{details}\n{content}"),
                (false, true) => details,
                (true, false) => content,
                (true, true) => String::new(),
            }
        }
    }
}

fn content_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(content_block)
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_block(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text { text } => Some(text.clone()),
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
        } => Some(format!(
            "tool call {name} ({id}): {}",
            truncate(&arguments.to_string(), 2_000)
        )),
        ContentBlock::Image { media_type, .. } => Some(format!(
            "image attachment ({media_type}; data retained in session)"
        )),
        ContentBlock::Thinking { .. } => None,
    }
}

fn concise_details(details: &serde_json::Value) -> String {
    if details.is_null() {
        return String::new();
    }
    truncate(&serde_json::to_string(details).unwrap_or_default(), 4_000)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut bounded = value.chars().take(max_chars).collect::<String>();
    bounded.push('…');
    bounded
}
