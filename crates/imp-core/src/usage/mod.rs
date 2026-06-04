use imp_llm::{model::ModelMeta, AssistantMessage, Cost, Message, Model, Usage};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::session::{SessionEntry, SessionManager};

/// Session custom entry type used for canonical usage accounting.
pub const USAGE_CUSTOM_TYPE: &str = "usage-record";

/// Current canonical usage record schema version.
pub const USAGE_RECORD_VERSION: u32 = 1;

/// Where a usage report came from when reading session history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageRecordSource {
    Canonical,
    LegacyAssistantMessage,
}

/// Stable request identity used for dedupe across copied/forked session history.
///
/// `request_id` is generated once per upstream model request and copied forward
/// with the canonical record. Global summaries should dedupe on this key so the
/// same request preserved in multiple session files is only counted once.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageDedupeKey {
    pub request_id: String,
}

/// Raw token accounting captured at request time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTokens {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl From<Usage> for UsageTokens {
    fn from(value: Usage) -> Self {
        Self {
            input: value.input_tokens,
            output: value.output_tokens,
            cache_read: value.cache_read_tokens,
            cache_write: value.cache_write_tokens,
        }
    }
}

impl From<&Usage> for UsageTokens {
    fn from(value: &Usage) -> Self {
        Self {
            input: value.input_tokens,
            output: value.output_tokens,
            cache_read: value.cache_read_tokens,
            cache_write: value.cache_write_tokens,
        }
    }
}

impl From<UsageTokens> for Usage {
    fn from(value: UsageTokens) -> Self {
        Self {
            input_tokens: value.input,
            output_tokens: value.output,
            cache_read_tokens: value.cache_read,
            cache_write_tokens: value.cache_write,
        }
    }
}

impl From<&UsageTokens> for Usage {
    fn from(value: &UsageTokens) -> Self {
        Self {
            input_tokens: value.input,
            output_tokens: value.output,
            cache_read_tokens: value.cache_read,
            cache_write_tokens: value.cache_write,
        }
    }
}

/// Stored dollar cost at request time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageCostBreakdown {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

impl From<Cost> for UsageCostBreakdown {
    fn from(value: Cost) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
            total: value.total,
        }
    }
}

impl From<&Cost> for UsageCostBreakdown {
    fn from(value: &Cost) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
            total: value.total,
        }
    }
}

impl From<UsageCostBreakdown> for Cost {
    fn from(value: UsageCostBreakdown) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
            total: value.total,
        }
    }
}

impl From<&UsageCostBreakdown> for Cost {
    fn from(value: &UsageCostBreakdown) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
            total: value.total,
        }
    }
}

/// Canonical usage record stored inside `SessionEntry::Custom`.
///
/// This schema is intentionally small and versioned. It captures stable request
/// identity, attribution for reporting, raw tokens, and stored cost so later
/// reporting doesn't need to recompute historical values from possibly changed
/// model pricing tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecordV1 {
    pub version: u32,
    pub request_id: String,
    pub recorded_at: u64,
    pub provider: String,
    pub model: String,
    pub session_id: Option<String>,
    pub session_path: Option<String>,
    pub assistant_message_id: Option<String>,
    pub turn_index: Option<u32>,
    pub usage: UsageTokens,
    pub cost: UsageCostBreakdown,
    pub source: UsageRecordSource,
}

impl UsageRecordV1 {
    pub fn new(
        request_id: impl Into<String>,
        recorded_at: u64,
        provider: impl Into<String>,
        model: impl Into<String>,
        usage: impl Into<UsageTokens>,
        cost: impl Into<UsageCostBreakdown>,
    ) -> Self {
        Self {
            version: USAGE_RECORD_VERSION,
            request_id: request_id.into(),
            recorded_at,
            provider: provider.into(),
            model: model.into(),
            session_id: None,
            session_path: None,
            assistant_message_id: None,
            turn_index: None,
            usage: usage.into(),
            cost: cost.into(),
            source: UsageRecordSource::Canonical,
        }
    }

    /// Stable dedupe identity for global usage rollups.
    pub fn dedupe_key(&self) -> UsageDedupeKey {
        UsageDedupeKey {
            request_id: self.request_id.clone(),
        }
    }

    pub fn usage_value(&self) -> Usage {
        Usage::from(&self.usage)
    }

    pub fn cost_value(&self) -> Cost {
        Cost::from(&self.cost)
    }

    pub fn with_session_context(
        mut self,
        session_id: Option<String>,
        session_path: Option<String>,
        assistant_message_id: Option<String>,
        turn_index: Option<u32>,
    ) -> Self {
        self.session_id = session_id;
        self.session_path = session_path;
        self.assistant_message_id = assistant_message_id;
        self.turn_index = turn_index;
        self
    }

    pub fn into_custom_data(self) -> Result<serde_json::Value> {
        serde_json::to_value(self).map_err(Into::into)
    }

    pub fn from_custom_data(value: serde_json::Value) -> Result<Self> {
        let record: Self = serde_json::from_value(value)?;
        if record.version != USAGE_RECORD_VERSION {
            return Err(Error::Session(format!(
                "unsupported usage record version: {}",
                record.version
            )));
        }
        Ok(record)
    }
}

/// Usage row returned by read helpers, including provenance and attribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageRecord {
    pub entry_id: String,
    pub parent_id: Option<String>,
    pub request_id: String,
    pub recorded_at: u64,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub session_path: Option<String>,
    pub assistant_message_id: Option<String>,
    pub turn_index: Option<u32>,
    pub usage: UsageTokens,
    pub cost: Option<UsageCostBreakdown>,
    pub source: UsageRecordSource,
}

impl SessionUsageRecord {
    pub fn dedupe_key(&self) -> UsageDedupeKey {
        UsageDedupeKey {
            request_id: self.request_id.clone(),
        }
    }

    pub fn usage_value(&self) -> Usage {
        Usage::from(&self.usage)
    }

    pub fn cost_value(&self) -> Option<Cost> {
        self.cost.as_ref().map(Cost::from)
    }
}

/// Aggregate totals across usage records.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageTotals {
    pub usage: Usage,
    pub cost: Cost,
    pub records: usize,
}

impl UsageTotals {
    pub fn add_record(&mut self, record: &SessionUsageRecord) {
        self.usage.add(&record.usage_value());
        if let Some(cost) = record.cost_value() {
            self.cost.add(&cost);
        }
        self.records += 1;
    }
}

/// Build a canonical usage record for a persisted assistant turn.
///
/// Returns `None` when the assistant message has no usage or when an equivalent
/// canonical record already exists for the same assistant turn.
pub fn canonical_usage_record_for_assistant_turn(
    session: &SessionManager,
    model: &Model,
    assistant_message_id: &str,
    turn_index: u32,
    message: &AssistantMessage,
) -> Option<UsageRecordV1> {
    canonical_usage_record_for_assistant_turn_with_model_meta(
        session,
        &model.meta,
        assistant_message_id,
        turn_index,
        message,
    )
}

/// Build a canonical usage record for a persisted assistant turn from model metadata.
pub fn canonical_usage_record_for_assistant_turn_with_model_meta(
    session: &SessionManager,
    model_meta: &ModelMeta,
    assistant_message_id: &str,
    turn_index: u32,
    message: &AssistantMessage,
) -> Option<UsageRecordV1> {
    let usage = message.usage.as_ref()?;
    let request_id = canonical_request_id(assistant_message_id);

    if session.has_canonical_usage_request_id(&request_id)
        || session.has_canonical_usage_for_assistant_message(assistant_message_id)
    {
        return None;
    }

    Some(
        UsageRecordV1::new(
            request_id,
            message.timestamp,
            model_meta.provider.clone(),
            model_meta.id.clone(),
            usage,
            usage.cost(&model_meta.pricing),
        )
        .with_session_context(
            session.session_id(),
            session.path().map(|p| p.display().to_string()),
            Some(assistant_message_id.to_string()),
            Some(turn_index),
        ),
    )
}

/// Read usage rows from a single session entry slice.
///
/// Canonical custom records are preferred. Legacy assistant-message usage is
/// only surfaced when no canonical usage record exists for the same
/// `assistant_message_id`, which preserves backward compatibility while
/// avoiding local double counting once canonical persistence lands.
pub fn usage_records_from_entries(entries: &[SessionEntry]) -> Vec<SessionUsageRecord> {
    let session_id = infer_session_id_from_entries(entries);
    let session_path = infer_session_path_from_entries(entries);

    let mut records = Vec::new();
    let mut canonical_assistant_ids = std::collections::HashSet::new();

    for entry in entries {
        if let Some(record) =
            canonical_usage_record_from_entry(entry, session_id.clone(), session_path.clone())
        {
            if let Some(assistant_message_id) = record.assistant_message_id.clone() {
                canonical_assistant_ids.insert(assistant_message_id);
            }
            records.push(record);
        }
    }

    let mut turn_index = 0u32;
    for entry in entries {
        if let SessionEntry::Message {
            id,
            parent_id,
            message: Message::Assistant(message),
        } = entry
        {
            if let Some(usage) = &message.usage {
                if !canonical_assistant_ids.contains(id) {
                    records.push(SessionUsageRecord {
                        entry_id: id.clone(),
                        parent_id: parent_id.clone(),
                        request_id: legacy_request_id(id),
                        recorded_at: message.timestamp,
                        provider: None,
                        model: None,
                        session_id: session_id.clone(),
                        session_path: session_path.clone(),
                        assistant_message_id: Some(id.clone()),
                        turn_index: Some(turn_index),
                        usage: UsageTokens::from(usage),
                        cost: None,
                        source: UsageRecordSource::LegacyAssistantMessage,
                    });
                }
                turn_index += 1;
            }
        }
    }

    records
}

/// Read usage rows from a session manager, attaching the session path when known.
pub fn usage_records_from_session(session: &SessionManager) -> Vec<SessionUsageRecord> {
    let session_id = session
        .session_id()
        .or_else(|| infer_session_id_from_entries(session.entries()));
    let session_path = session
        .path()
        .map(|p| p.display().to_string())
        .or_else(|| infer_session_path_from_entries(session.entries()));

    let mut records = Vec::new();
    let mut canonical_assistant_ids = std::collections::HashSet::new();

    for entry in session.entries() {
        if let Some(record) =
            canonical_usage_record_from_entry(entry, session_id.clone(), session_path.clone())
        {
            if let Some(assistant_message_id) = record.assistant_message_id.clone() {
                canonical_assistant_ids.insert(assistant_message_id);
            }
            records.push(record);
        }
    }

    let mut turn_index = 0u32;
    for entry in session.entries() {
        if let SessionEntry::Message {
            id,
            parent_id,
            message: Message::Assistant(message),
        } = entry
        {
            if let Some(usage) = &message.usage {
                if !canonical_assistant_ids.contains(id) {
                    records.push(SessionUsageRecord {
                        entry_id: id.clone(),
                        parent_id: parent_id.clone(),
                        request_id: legacy_request_id(id),
                        recorded_at: message.timestamp,
                        provider: None,
                        model: None,
                        session_id: session_id.clone(),
                        session_path: session_path.clone(),
                        assistant_message_id: Some(id.clone()),
                        turn_index: Some(turn_index),
                        usage: UsageTokens::from(usage),
                        cost: None,
                        source: UsageRecordSource::LegacyAssistantMessage,
                    });
                }
                turn_index += 1;
            }
        }
    }

    records
}

/// Return a deduped record set while preserving a stable canonical ordering.
///
/// Records are sorted so canonical rows win over legacy fallbacks, then the
/// earliest observation of a request is kept. This gives downstream reporting a
/// deterministic per-request representative row for grouping by model, day, or
/// session.
pub fn dedupe_usage_records(records: &[SessionUsageRecord]) -> Vec<SessionUsageRecord> {
    let mut sorted = records.to_vec();
    sorted.sort_by(usage_record_preference);

    let mut seen = std::collections::HashSet::new();
    sorted
        .into_iter()
        .filter(|record| seen.insert(record.dedupe_key()))
        .collect()
}

/// Sum usage rows without dedupe.
pub fn aggregate_usage(records: &[SessionUsageRecord]) -> UsageTotals {
    let mut totals = UsageTotals::default();
    for record in records {
        totals.add_record(record);
    }
    totals
}

/// Sum usage rows while deduping copied/forked history by stable request id.
pub fn aggregate_usage_deduped(records: &[SessionUsageRecord]) -> UsageTotals {
    let deduped = dedupe_usage_records(records);
    aggregate_usage(&deduped)
}

fn usage_record_preference(a: &SessionUsageRecord, b: &SessionUsageRecord) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    usage_source_rank(a.source)
        .cmp(&usage_source_rank(b.source))
        .then_with(|| a.recorded_at.cmp(&b.recorded_at))
        .then_with(|| a.session_id.cmp(&b.session_id))
        .then_with(|| a.session_path.cmp(&b.session_path))
        .then_with(|| a.assistant_message_id.cmp(&b.assistant_message_id))
        .then_with(|| a.entry_id.cmp(&b.entry_id))
        .then(Ordering::Equal)
}

fn usage_source_rank(source: UsageRecordSource) -> u8 {
    match source {
        UsageRecordSource::Canonical => 0,
        UsageRecordSource::LegacyAssistantMessage => 1,
    }
}

/// Build a canonical session custom entry for persistence.
pub fn usage_record_entry(
    entry_id: impl Into<String>,
    record: UsageRecordV1,
) -> Result<SessionEntry> {
    Ok(SessionEntry::Custom {
        id: entry_id.into(),
        parent_id: None,
        custom_type: USAGE_CUSTOM_TYPE.to_string(),
        data: record.into_custom_data()?,
    })
}

fn canonical_usage_record_from_entry(
    entry: &SessionEntry,
    fallback_session_id: Option<String>,
    fallback_session_path: Option<String>,
) -> Option<SessionUsageRecord> {
    let SessionEntry::Custom {
        id,
        parent_id,
        custom_type,
        data,
    } = entry
    else {
        return None;
    };

    if custom_type != USAGE_CUSTOM_TYPE {
        return None;
    }

    let record = UsageRecordV1::from_custom_data(data.clone()).ok()?;
    Some(SessionUsageRecord {
        entry_id: id.clone(),
        parent_id: parent_id.clone(),
        request_id: record.request_id,
        recorded_at: record.recorded_at,
        provider: Some(record.provider),
        model: Some(record.model),
        session_id: record.session_id.or(fallback_session_id),
        session_path: record.session_path.or(fallback_session_path),
        assistant_message_id: record.assistant_message_id,
        turn_index: record.turn_index,
        usage: record.usage,
        cost: Some(record.cost),
        source: record.source,
    })
}

fn infer_session_id_from_entries(entries: &[SessionEntry]) -> Option<String> {
    entries.iter().find_map(|entry| {
        let SessionEntry::Custom {
            custom_type, data, ..
        } = entry
        else {
            return None;
        };

        if custom_type != USAGE_CUSTOM_TYPE {
            return None;
        }

        UsageRecordV1::from_custom_data(data.clone())
            .ok()
            .and_then(|record| record.session_id)
    })
}

fn infer_session_path_from_entries(entries: &[SessionEntry]) -> Option<String> {
    entries.iter().find_map(|entry| {
        let SessionEntry::Custom {
            custom_type, data, ..
        } = entry
        else {
            return None;
        };

        if custom_type != USAGE_CUSTOM_TYPE {
            return None;
        }

        UsageRecordV1::from_custom_data(data.clone())
            .ok()
            .and_then(|record| record.session_path)
    })
}

fn canonical_request_id(assistant_message_id: &str) -> String {
    format!("assistant:{assistant_message_id}")
}

fn legacy_request_id(assistant_message_id: &str) -> String {
    format!("legacy-assistant:{assistant_message_id}")
}

#[cfg(test)]
mod tests;
