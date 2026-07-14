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
pub struct FactCoverage {
    pub fact_id: String,
    pub summary_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionDocument {
    pub version: u32,
    pub summary: String,
    pub fact_coverage: Vec<FactCoverage>,
}

pub fn validate_document_shape(document: &CompactionDocument) -> Result<(), String> {
    if document.version != COMPACTION_RECORD_VERSION {
        return Err(format!(
            "unsupported compaction document version {}",
            document.version
        ));
    }
    if document.summary.trim().is_empty() {
        return Err("compaction document summary is empty".to_string());
    }
    Ok(())
}

pub fn validate_document(
    state: &ContinuationState,
    document: &CompactionDocument,
) -> Result<(), String> {
    validate_document_shape(document)?;
    let missing = state
        .required_fact_ids()
        .into_iter()
        .filter(|id| {
            !document.fact_coverage.iter().any(|coverage| {
                coverage.fact_id == *id
                    && !coverage.summary_excerpt.trim().is_empty()
                    && document.summary.contains(coverage.summary_excerpt.trim())
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::state::{ContinuationState, FactKind, StateFact};

    fn required_state() -> ContinuationState {
        ContinuationState {
            version: 1,
            facts: vec![StateFact {
                id: "fact-1".into(),
                kind: FactKind::Constraint,
                text: "preserve the constraint".into(),
                source_entry_id: "entry-1".into(),
                required: true,
            }],
        }
    }

    #[test]
    fn validation_requires_fact_excerpt_present_in_summary() {
        let state = required_state();
        let missing = CompactionDocument {
            version: COMPACTION_RECORD_VERSION,
            summary: "A useful summary".into(),
            fact_coverage: vec![FactCoverage {
                fact_id: "fact-1".into(),
                summary_excerpt: "not in summary".into(),
            }],
        };
        assert!(validate_document(&state, &missing).is_err());

        let covered = CompactionDocument {
            version: COMPACTION_RECORD_VERSION,
            summary: "A useful summary preserves the constraint.".into(),
            fact_coverage: vec![FactCoverage {
                fact_id: "fact-1".into(),
                summary_excerpt: "preserves the constraint".into(),
            }],
        };
        assert!(validate_document(&state, &covered).is_ok());
    }
}
