use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::state::ContinuationState;
use crate::context::estimate_tokens;
use crate::error::{Error, Result};
use crate::session::{ActiveMessageSource, ActiveSessionMessage};

pub const CHECKPOINT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointSource {
    pub entry_ids: Vec<String>,
    pub uncovered_start: usize,
    pub uncovered_tokens: u32,
    pub source_fingerprint: String,
    pub inherited_continuation: Option<ContinuationState>,
}

impl CheckpointSource {
    pub fn is_due(&self, interval_tokens: u32) -> bool {
        self.uncovered_tokens >= interval_tokens
    }
}

pub fn checkpoint_source(
    active: &[ActiveSessionMessage],
    previous: Option<&CompactionCheckpoint>,
) -> Result<CheckpointSource> {
    let entry_ids = active.iter().map(entry_id).collect::<Vec<_>>();
    let (uncovered_start, inherited_continuation) = match previous {
        Some(checkpoint) => (covered_prefix_len(&entry_ids, checkpoint)?, None),
        None => inherited_v2_state(active),
    };
    let uncovered_tokens = active[uncovered_start..]
        .iter()
        .map(|entry| estimate_tokens(&serde_json::to_string(&entry.message).unwrap_or_default()))
        .sum();
    Ok(CheckpointSource {
        source_fingerprint: fingerprint(&entry_ids),
        entry_ids,
        uncovered_start,
        uncovered_tokens,
        inherited_continuation,
    })
}

fn inherited_v2_state(active: &[ActiveSessionMessage]) -> (usize, Option<ContinuationState>) {
    let Some(first) = active.first() else {
        return (0, None);
    };
    let ActiveMessageSource::Compaction { continuation, .. } = &first.source else {
        return (0, None);
    };
    (1, continuation.clone())
}

fn covered_prefix_len(entry_ids: &[String], checkpoint: &CompactionCheckpoint) -> Result<usize> {
    let covered = checkpoint.covered_entry_ids.as_slice();
    if entry_ids.get(..covered.len()) == Some(covered) {
        Ok(covered.len())
    } else {
        Err(Error::Config(
            "compaction checkpoint does not cover the active branch prefix".to_string(),
        ))
    }
}

fn entry_id(entry: &ActiveSessionMessage) -> String {
    match &entry.source {
        ActiveMessageSource::Compaction { entry_id, .. }
        | ActiveMessageSource::Message { entry_id } => entry_id.clone(),
    }
}

fn fingerprint(entry_ids: &[String]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in entry_ids.iter().flat_map(|id| id.bytes().chain([0])) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub version: u32,
    pub id: String,
    pub previous_checkpoint_id: Option<String>,
    pub covered_entry_ids: Vec<String>,
    pub source_fingerprint: String,
    pub continuation: ContinuationState,
    pub summary: String,
    pub model_id: String,
    pub provider_id: String,
    pub thinking: String,
    pub source_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct CheckpointStore {
    path: PathBuf,
}

impl CheckpointStore {
    pub fn for_session(session_path: &Path) -> Self {
        let file_name = session_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session.jsonl".to_string());
        Self {
            path: session_path.with_file_name(format!("{file_name}.compaction.json")),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<CompactionCheckpoint>> {
        let content = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let checkpoint = serde_json::from_str::<CompactionCheckpoint>(&content)?;
        checkpoint.validate()?;
        Ok(Some(checkpoint))
    }

    pub fn publish(&self, checkpoint: &CompactionCheckpoint) -> Result<()> {
        checkpoint.validate()?;
        let parent = self.path.parent().ok_or_else(|| {
            Error::Config("compaction checkpoint path has no parent directory".to_string())
        })?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            self.path.file_name().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let result = write_and_replace(&temporary, &self.path, checkpoint);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl CompactionCheckpoint {
    pub fn validate(&self) -> Result<()> {
        if self.version != CHECKPOINT_VERSION {
            return Err(Error::Config(format!(
                "unsupported compaction checkpoint version {}",
                self.version
            )));
        }
        if self.id.trim().is_empty()
            || self.source_fingerprint.trim().is_empty()
            || self.summary.trim().is_empty()
            || self.covered_entry_ids.is_empty()
        {
            return Err(Error::Config(
                "compaction checkpoint is missing required state".to_string(),
            ));
        }
        Ok(())
    }
}

fn write_and_replace(
    temporary: &Path,
    destination: &Path,
    checkpoint: &CompactionCheckpoint,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    serde_json::to_writer_pretty(&mut file, checkpoint)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, destination)?;
    sync_parent(destination)
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = path.parent().ok_or_else(|| {
            Error::Config("compaction checkpoint path has no parent directory".to_string())
        })?;
        let directory = fs::File::open(parent)?;
        directory.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::state::CONTINUATION_STATE_VERSION;
    use crate::session::{ActiveMessageSource, ActiveSessionMessage};
    use imp_llm::Message;

    fn checkpoint(summary: &str) -> CompactionCheckpoint {
        CompactionCheckpoint {
            version: CHECKPOINT_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            previous_checkpoint_id: None,
            covered_entry_ids: vec!["entry-1".into()],
            source_fingerprint: "entry-1".into(),
            continuation: ContinuationState {
                version: CONTINUATION_STATE_VERSION,
                facts: Vec::new(),
            },
            summary: summary.into(),
            model_id: "gpt-5.6-luna".into(),
            provider_id: "openai".into(),
            thinking: "xhigh".into(),
            source_tokens: 128_000,
        }
    }

    fn active(id: &str, text: &str) -> ActiveSessionMessage {
        ActiveSessionMessage {
            source: ActiveMessageSource::Message {
                entry_id: id.to_string(),
            },
            message: Message::user(text),
        }
    }

    #[test]
    fn source_counts_only_entries_after_valid_checkpoint_prefix() {
        let entries = vec![active("one", "small"), active("two", &"x".repeat(2_000))];
        let mut previous = checkpoint("first");
        previous.covered_entry_ids = vec!["one".into()];

        let source = checkpoint_source(&entries, Some(&previous)).unwrap();

        assert_eq!(source.uncovered_start, 1);
        assert!(source.uncovered_tokens > 400);
        assert!(source.is_due(400));
        assert!(!source.is_due(10_000));
    }

    #[test]
    fn source_rejects_checkpoint_from_different_branch() {
        let entries = vec![active("other", "small")];
        let previous = checkpoint("first");

        let error = checkpoint_source(&entries, Some(&previous)).unwrap_err();

        assert!(error.to_string().contains("active branch prefix"));
    }

    #[test]
    fn checkpoint_publish_replaces_only_after_validation() {
        let temp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::for_session(&temp.path().join("session.jsonl"));
        store.publish(&checkpoint("first")).unwrap();
        assert_eq!(store.load().unwrap().unwrap().summary, "first");

        let mut invalid = checkpoint("");
        invalid.previous_checkpoint_id = Some("previous".into());
        assert!(store.publish(&invalid).is_err());
        assert_eq!(store.load().unwrap().unwrap().summary, "first");
    }

    #[test]
    fn checkpoint_path_is_session_owned() {
        let store = CheckpointStore::for_session(Path::new("/tmp/abc.jsonl"));
        assert_eq!(store.path(), Path::new("/tmp/abc.jsonl.compaction.json"));
    }
}
