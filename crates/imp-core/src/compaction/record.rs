use serde::{Deserialize, Serialize};

use super::state::ContinuationState;

pub const COMPACTION_RECORD_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompactionTrigger {
    Manual,
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub version: u32,
    pub trigger: CompactionTrigger,
    pub summary: String,
    pub source_entry_ids: Vec<String>,
    pub preserved_entry_ids: Vec<String>,
    pub first_kept_id: String,
    pub continuation: ContinuationState,
    pub model_id: String,
    pub provider_id: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionDocument {
    pub version: u32,
    pub summary: String,
    pub acknowledged_fact_ids: Vec<String>,
}

pub fn validate_document(
    state: &ContinuationState,
    document: &CompactionDocument,
) -> Result<(), String> {
    if document.version != COMPACTION_RECORD_VERSION {
        return Err(format!(
            "unsupported compaction document version {}",
            document.version
        ));
    }
    if document.summary.trim().is_empty() {
        return Err("compaction document summary is empty".to_string());
    }
    let missing = state
        .required_fact_ids()
        .into_iter()
        .filter(|id| {
            !document
                .acknowledged_fact_ids
                .iter()
                .any(|acknowledged| acknowledged == id)
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "compaction document omitted required facts: {}",
            missing.join(", ")
        ))
    }
}
