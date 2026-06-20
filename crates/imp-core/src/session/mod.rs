use std::collections::HashMap;
use std::path::{Path, PathBuf};

use imp_llm::{
    truncate_chars_with_suffix, AssistantMessage, ContentBlock, Message, Model, ToolResultMessage,
    UserMessage,
};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, RecoveryCheckpoint};
use crate::error::Result;
use crate::usage::{
    canonical_usage_record_for_assistant_turn_with_model_meta, usage_record_entry,
    usage_records_from_session, SessionUsageRecord, UsageRecordV1, USAGE_CUSTOM_TYPE,
};
use listing::{read_first_line, read_session_info, recent_session_files, session_info_matches};
use summary::{derive_session_summary, extract_text, preferred_title_candidate};
#[cfg(test)]
use summary::{literal_topic_title, summarize_session_title};

mod listing;
mod summary;

pub const CHECKPOINT_CUSTOM_TYPE: &str = "checkpoint-record";
pub const CHECKPOINT_RECORD_VERSION: u32 = 1;
pub const RECOVERY_CHECKPOINT_CUSTOM_TYPE: &str = "recovery-checkpoint";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCheckpointRecord {
    pub version: u32,
    pub checkpoint_id: String,
    pub created_at: u64,
    pub label: Option<String>,
    pub files: Vec<String>,
}

const SESSION_META_VERSION: u32 = 1;

/// A single entry in the session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntry {
    #[serde(rename = "header")]
    Header {
        version: u32,
        created_at: u64,
        cwd: String,
    },
    #[serde(rename = "message")]
    Message {
        id: String,
        parent_id: Option<String>,
        message: Message,
    },
    #[serde(rename = "compaction")]
    Compaction {
        id: String,
        parent_id: Option<String>,
        summary: String,
        first_kept_id: String,
        #[serde(default)]
        tokens_before: u32,
        #[serde(default)]
        tokens_after: u32,
    },
    #[serde(rename = "custom")]
    Custom {
        id: String,
        parent_id: Option<String>,
        custom_type: String,
        data: serde_json::Value,
    },
    #[serde(rename = "label")]
    Label { entry_id: String, label: String },
    #[serde(rename = "session-meta")]
    SessionMeta {
        version: u32,
        name: Option<String>,
        summary: Option<String>,
    },
}

impl SessionEntry {
    /// Get the id of this entry, if it has one (Header and Label don't).
    pub fn id(&self) -> Option<&str> {
        match self {
            SessionEntry::Header { .. }
            | SessionEntry::Label { .. }
            | SessionEntry::SessionMeta { .. } => None,
            SessionEntry::Message { id, .. }
            | SessionEntry::Compaction { id, .. }
            | SessionEntry::Custom { id, .. } => Some(id),
        }
    }

    /// Get the parent_id of this entry, if it has one.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Header { .. }
            | SessionEntry::Label { .. }
            | SessionEntry::SessionMeta { .. } => None,
            SessionEntry::Message { parent_id, .. }
            | SessionEntry::Compaction { parent_id, .. }
            | SessionEntry::Custom { parent_id, .. } => parent_id.as_deref(),
        }
    }
}

/// A node in the session tree.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub entry: SessionEntry,
    pub children: Vec<TreeNode>,
}

/// Summary of a session for listing.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub first_message: Option<String>,
    pub last_message: Option<String>,
    pub name: Option<String>,
    pub summary: Option<String>,
}

pub fn checkpoint_record_entry(
    entry_id: impl Into<String>,
    record: SessionCheckpointRecord,
) -> Result<SessionEntry> {
    Ok(SessionEntry::Custom {
        id: entry_id.into(),
        parent_id: None,
        custom_type: CHECKPOINT_CUSTOM_TYPE.to_string(),
        data: serde_json::to_value(record)?,
    })
}

pub fn recovery_checkpoint_entry(
    entry_id: impl Into<String>,
    checkpoint: RecoveryCheckpoint,
) -> Result<SessionEntry> {
    Ok(SessionEntry::Custom {
        id: entry_id.into(),
        parent_id: None,
        custom_type: RECOVERY_CHECKPOINT_CUSTOM_TYPE.to_string(),
        data: serde_json::to_value(checkpoint)?,
    })
}

impl SessionInfo {
    /// A short, single-line chat title derived from persisted session metadata or message history.
    pub fn title(&self, max_chars: usize) -> Option<String> {
        if let Some(name) = self
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| truncate_chars_with_suffix(name.trim(), max_chars, "…"))
        {
            return Some(name);
        }

        preferred_title_candidate(
            self.first_message.as_deref(),
            self.summary.as_deref(),
            max_chars,
        )
    }
}

/// Manages a single session's entries and persistence.
///
/// Raw persisted entries are always retained in `entries`. Active model-visible
/// history may differ from the raw branch when a `SessionEntry::Compaction`
/// exists on the current branch. In that case, callers should prefer
/// `get_active_messages()` over `get_messages()` when assembling context for an
/// LLM request.
#[derive(Debug, Clone)]
pub struct SessionManager {
    entries: Vec<SessionEntry>,
    path: Option<PathBuf>,
    leaf_id: Option<String>,
    session_name: Option<String>,
    session_summary: Option<String>,
}

fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl SessionManager {
    /// Create a new session. Writes the header to disk immediately.
    pub fn new(cwd: &Path, session_dir: &Path) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let path = session_dir.join(format!("{session_id}.jsonl"));
        let header = SessionEntry::Header {
            version: 1,
            created_at: imp_llm::now(),
            cwd: cwd.to_string_lossy().to_string(),
        };

        // Write header to disk immediately
        {
            use std::io::Write;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&path)?;
            let line = serde_json::to_string(&header)?;
            writeln!(file, "{line}")?;
        }

        Ok(Self {
            entries: vec![header],
            path: Some(path),
            leaf_id: None,
            session_name: None,
            session_summary: None,
        })
    }

    /// Open an existing session file, skipping malformed lines.
    pub fn open(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut entries = Vec::new();
        let mut last_id = None;

        let mut session_name = None;
        let mut session_summary = None;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEntry>(line) {
                Ok(entry) => {
                    if let Some(id) = entry.id() {
                        last_id = Some(id.to_string());
                    }
                    if let SessionEntry::SessionMeta { name, summary, .. } = &entry {
                        session_name = name.clone();
                        session_summary = summary.clone();
                    }
                    entries.push(entry);
                }
                Err(_e) => {
                    // Keep session loading side-effect free for embedded callers like the TUI.
                    // Malformed lines are skipped so resume/continue can still recover usable history.
                }
            }
        }

        Ok(Self {
            entries,
            path: Some(path.to_path_buf()),
            leaf_id: last_id,
            session_name,
            session_summary,
        })
    }

    /// In-memory session (no persistence).
    pub fn in_memory() -> Self {
        Self {
            entries: Vec::new(),
            path: None,
            leaf_id: None,
            session_name: None,
            session_summary: None,
        }
    }

    /// In-memory session seeded with a linear message history.
    pub fn in_memory_with_messages(messages: Vec<Message>) -> Self {
        let mut session = Self::in_memory();
        for message in messages {
            let _ = session.append(SessionEntry::Message {
                id: uuid::Uuid::new_v4().to_string(),
                parent_id: None,
                message,
            });
        }
        session
    }

    /// Find the most recently modified session for a given cwd.
    pub fn continue_recent(cwd: &Path, session_dir: &Path) -> Result<Option<Self>> {
        if !session_dir.exists() {
            return Ok(None);
        }

        let cwd_str = cwd.to_string_lossy().to_string();
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

        for dir_entry in std::fs::read_dir(session_dir)? {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            // Check modification time first (cheap) before parsing
            let modified = dir_entry
                .metadata()?
                .modified()
                .unwrap_or(std::time::UNIX_EPOCH);

            // Only parse if this could be newer than our current best
            if best.as_ref().is_none_or(|(t, _)| modified > *t) {
                // Read just the first line to check cwd without parsing the whole file
                if let Ok(first_line) = read_first_line(&path) {
                    if let Ok(SessionEntry::Header { cwd, .. }) =
                        serde_json::from_str::<SessionEntry>(&first_line).as_ref()
                    {
                        if *cwd == cwd_str {
                            best = Some((modified, path));
                        }
                    }
                }
            }
        }

        match best {
            Some((_, path)) => Ok(Some(Self::open(&path)?)),
            None => Ok(None),
        }
    }

    /// Get the session name.
    pub fn name(&self) -> Option<&str> {
        self.session_name.as_deref()
    }

    /// Get the session summary.
    pub fn summary(&self) -> Option<&str> {
        self.session_summary.as_deref()
    }

    /// Set the session name.
    pub fn set_name(&mut self, name: &str) {
        self.session_name = Some(name.to_string());
        let _ = self.persist_session_meta();
    }

    /// Set the session summary.
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        let summary = summary.into();
        self.session_summary = Some(summary);
        let _ = self.persist_session_meta();
    }

    /// Clear the session summary.
    pub fn clear_summary(&mut self) {
        self.session_summary = None;
        let _ = self.persist_session_meta();
    }

    /// A short, single-line chat title derived from persisted session metadata or message history.
    pub fn title(&self, max_chars: usize) -> Option<String> {
        if let Some(name) = self
            .name()
            .filter(|name| !name.trim().is_empty())
            .map(|name| truncate_chars_with_suffix(name.trim(), max_chars, "…"))
        {
            return Some(name);
        }

        let first_prompt = self.entries.iter().find_map(|entry| match entry {
            SessionEntry::Message { message, .. } => extract_text(message),
            _ => None,
        });
        let summary = self
            .summary()
            .filter(|summary| !summary.trim().is_empty())
            .map(str::to_string)
            .or_else(|| derive_session_summary(&self.entries));

        preferred_title_candidate(first_prompt.as_deref(), summary.as_deref(), max_chars)
    }

    fn persist_session_meta(&mut self) -> Result<()> {
        self.append(SessionEntry::SessionMeta {
            version: SESSION_META_VERSION,
            name: self.session_name.clone(),
            summary: self.session_summary.clone(),
        })
    }

    fn refresh_derived_summary(&mut self) {
        let derived = derive_session_summary(&self.entries);
        if derived != self.session_summary {
            self.session_summary = derived;
            let _ = self.persist_session_meta();
        }
    }

    /// Append an entry. Sets parent_id to current leaf_id, updates leaf_id,
    /// and writes to file if persisted.
    pub fn append(&mut self, mut entry: SessionEntry) -> Result<()> {
        // Set parent_id on entries that support it
        match &mut entry {
            SessionEntry::Message { parent_id, .. }
            | SessionEntry::Compaction { parent_id, .. }
            | SessionEntry::Custom { parent_id, .. } => {
                *parent_id = self.leaf_id.clone();
            }
            SessionEntry::Header { .. }
            | SessionEntry::Label { .. }
            | SessionEntry::SessionMeta { .. } => {}
        }

        // Update leaf_id
        if let Some(id) = entry.id() {
            self.leaf_id = Some(id.to_string());
        }

        // Write to file
        if let Some(ref path) = self.path {
            use std::io::Write;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            set_private_permissions(path)?;
            let line = serde_json::to_string(&entry)?;
            writeln!(file, "{line}")?;
        }

        self.entries.push(entry);
        Ok(())
    }

    /// Append an assistant turn and, when available, its canonical usage record.
    pub fn append_assistant_turn(
        &mut self,
        model: &Model,
        turn_index: u32,
        message: AssistantMessage,
    ) -> Result<(String, Option<String>)> {
        self.append_assistant_turn_with_model_meta(&model.meta, turn_index, message)
    }

    /// Append an assistant turn and, when available, its canonical usage record.
    pub fn append_assistant_turn_with_model_meta(
        &mut self,
        model_meta: &imp_llm::model::ModelMeta,
        turn_index: u32,
        message: AssistantMessage,
    ) -> Result<(String, Option<String>)> {
        let assistant_message_id = uuid::Uuid::new_v4().to_string();
        self.append(SessionEntry::Message {
            id: assistant_message_id.clone(),
            parent_id: None,
            message: Message::Assistant(message.clone()),
        })?;

        let usage_entry_id = self.append_canonical_usage_for_assistant_turn_with_model_meta(
            model_meta,
            &assistant_message_id,
            turn_index,
            &message,
        )?;

        self.refresh_derived_summary();

        Ok((assistant_message_id, usage_entry_id))
    }

    /// Append a tool result message and return the persisted entry id.
    pub fn append_tool_result_message(&mut self, result: ToolResultMessage) -> Result<String> {
        let entry_id = uuid::Uuid::new_v4().to_string();
        self.append(SessionEntry::Message {
            id: entry_id.clone(),
            parent_id: None,
            message: Message::ToolResult(result),
        })?;
        Ok(entry_id)
    }

    /// Append a recovery checkpoint custom entry and return the persisted entry id.
    pub fn append_recovery_checkpoint(&mut self, checkpoint: RecoveryCheckpoint) -> Result<String> {
        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = recovery_checkpoint_entry(entry_id.clone(), checkpoint)?;
        self.append(entry)?;
        Ok(entry_id)
    }

    /// Persist the session entries implied by an agent event.
    ///
    /// Returns a short description of what was written so callers can surface
    /// best-effort persistence diagnostics without owning the persistence logic.
    pub fn persist_agent_event_entries(
        &mut self,
        model: &Model,
        event: &AgentEvent,
    ) -> Result<Vec<&'static str>> {
        self.persist_agent_event_entries_with_model_meta(&model.meta, event)
    }

    /// Persist the session entries implied by an agent event.
    ///
    /// Returns a short description of what was written so callers can surface
    /// best-effort persistence diagnostics without owning the persistence logic.
    pub fn persist_agent_event_entries_with_model_meta(
        &mut self,
        model_meta: &imp_llm::model::ModelMeta,
        event: &AgentEvent,
    ) -> Result<Vec<&'static str>> {
        let mut persisted = Vec::new();

        match event {
            AgentEvent::ToolExecutionEnd { result, .. } => {
                self.append_tool_result_message(result.clone())?;
                persisted.push("tool result");
            }
            AgentEvent::TurnEnd { index, message, .. } => {
                let (_assistant_id, usage_entry_id) = self.append_assistant_turn_with_model_meta(
                    model_meta,
                    *index,
                    message.clone(),
                )?;
                persisted.push("assistant message");
                if usage_entry_id.is_some() {
                    persisted.push("canonical usage");
                }
            }
            AgentEvent::RecoveryCheckpoint { checkpoint } => {
                self.append_recovery_checkpoint(checkpoint.clone())?;
                persisted.push("recovery checkpoint");
            }
            _ => {}
        }

        Ok(persisted)
    }

    /// Append a canonical usage entry for an assistant turn, if the turn reports usage
    /// and no equivalent canonical record already exists.
    ///
    /// This is best-effort metadata persistence: callers should treat errors as
    /// non-fatal to the main agent flow.
    pub fn append_canonical_usage_for_assistant_turn(
        &mut self,
        model: &Model,
        assistant_message_id: &str,
        turn_index: u32,
        message: &AssistantMessage,
    ) -> Result<Option<String>> {
        self.append_canonical_usage_for_assistant_turn_with_model_meta(
            &model.meta,
            assistant_message_id,
            turn_index,
            message,
        )
    }

    /// Append a canonical usage entry for an assistant turn, if the turn reports usage
    /// and no equivalent canonical record already exists.
    ///
    /// This is best-effort metadata persistence: callers should treat errors as
    /// non-fatal to the main agent flow.
    pub fn append_canonical_usage_for_assistant_turn_with_model_meta(
        &mut self,
        model_meta: &imp_llm::model::ModelMeta,
        assistant_message_id: &str,
        turn_index: u32,
        message: &AssistantMessage,
    ) -> Result<Option<String>> {
        let Some(record) = canonical_usage_record_for_assistant_turn_with_model_meta(
            self,
            model_meta,
            assistant_message_id,
            turn_index,
            message,
        ) else {
            return Ok(None);
        };

        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = usage_record_entry(entry_id.clone(), record)?;
        self.append(entry)?;
        Ok(Some(entry_id))
    }

    /// Read canonical usage rows attached to this session.
    pub fn usage_records(&self) -> Vec<SessionUsageRecord> {
        usage_records_from_session(self)
    }

    pub fn append_checkpoint_record(&mut self, record: SessionCheckpointRecord) -> Result<String> {
        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = checkpoint_record_entry(entry_id.clone(), record)?;
        self.append(entry)?;
        Ok(entry_id)
    }

    pub fn checkpoint_records(&self) -> Vec<SessionCheckpointRecord> {
        self.entries
            .iter()
            .filter_map(|entry| {
                let SessionEntry::Custom {
                    custom_type, data, ..
                } = entry
                else {
                    return None;
                };

                if custom_type != CHECKPOINT_CUSTOM_TYPE {
                    return None;
                }

                serde_json::from_value::<SessionCheckpointRecord>(data.clone()).ok()
            })
            .collect()
    }

    pub fn find_checkpoint_record(&self, needle: &str) -> Option<SessionCheckpointRecord> {
        self.checkpoint_records().into_iter().find(|record| {
            record.checkpoint_id == needle || record.label.as_deref() == Some(needle)
        })
    }

    pub fn restore_checkpoint(
        &self,
        checkpoint_state: &crate::tools::CheckpointState,
        needle: &str,
    ) -> Result<Vec<PathBuf>> {
        let Some(record) = self.find_checkpoint_record(needle) else {
            return Ok(Vec::new());
        };
        checkpoint_state
            .restore_checkpoint(&record.checkpoint_id)
            .map_err(Into::into)
    }

    /// Check whether a canonical usage record already exists for the given request id.
    pub fn has_canonical_usage_request_id(&self, request_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            let SessionEntry::Custom {
                custom_type, data, ..
            } = entry
            else {
                return false;
            };

            if custom_type != USAGE_CUSTOM_TYPE {
                return false;
            }

            UsageRecordV1::from_custom_data(data.clone())
                .map(|record| record.request_id == request_id)
                .unwrap_or(false)
        })
    }

    /// Check whether a canonical usage record already exists for the given assistant turn.
    pub fn has_canonical_usage_for_assistant_message(&self, assistant_message_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            let SessionEntry::Custom {
                custom_type, data, ..
            } = entry
            else {
                return false;
            };

            if custom_type != USAGE_CUSTOM_TYPE {
                return false;
            }

            UsageRecordV1::from_custom_data(data.clone())
                .ok()
                .and_then(|record| record.assistant_message_id)
                .as_deref()
                == Some(assistant_message_id)
        })
    }

    /// Walk parent_ids from leaf_id to root, return raw entries in chronological order.
    ///
    /// This is the durable branch as persisted on disk. It may include
    /// `SessionEntry::Compaction` markers plus raw pre-compaction messages.
    /// Callers building model-visible context should prefer
    /// `get_active_messages()`.
    pub fn get_branch(&self) -> Vec<&SessionEntry> {
        let Some(ref leaf) = self.leaf_id else {
            // No messages yet — return just the header if present
            return self
                .entries
                .iter()
                .filter(|e| matches!(e, SessionEntry::Header { .. }))
                .collect();
        };

        // Build id -> entry index for fast lookups
        let id_map: HashMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.id().map(|id| (id, i)))
            .collect();

        // Walk from leaf to root
        let mut branch = Vec::new();
        let mut current = Some(leaf.as_str());

        while let Some(id) = current {
            if let Some(&idx) = id_map.get(id) {
                let entry = &self.entries[idx];
                branch.push(entry);
                current = entry.parent_id();
            } else {
                break;
            }
        }

        // Include the header
        for entry in &self.entries {
            if matches!(entry, SessionEntry::Header { .. }) {
                branch.push(entry);
                break;
            }
        }

        branch.reverse();
        branch
    }

    /// Get raw message entries for the current branch.
    ///
    /// This reflects the durable branch exactly and intentionally ignores
    /// compaction semantics. For model-visible history after a compaction,
    /// prefer `get_active_messages()`.
    pub fn get_messages(&self) -> Vec<&Message> {
        self.get_branch()
            .into_iter()
            .filter_map(|e| match e {
                SessionEntry::Message { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// Return the latest compaction entry on the active branch, if any.
    pub fn latest_compaction(&self) -> Option<&SessionEntry> {
        self.get_branch()
            .into_iter()
            .rev()
            .find(|entry| matches!(entry, SessionEntry::Compaction { .. }))
    }

    /// Build the model-visible message history for the active branch.
    ///
    /// Compaction semantics are branch-local and replacement-based:
    /// - if there is no compaction entry on the branch, this returns the raw
    ///   branch messages;
    /// - if a compaction entry exists, all raw messages before that boundary are
    ///   replaced by a synthetic user summary message derived from the latest
    ///   compaction entry, followed by the raw messages from `first_kept_id`
    ///   onward that are still on the active branch.
    ///
    /// Raw persisted entries remain intact on disk and are still available via
    /// `get_branch()` / `get_messages()`.
    pub fn get_active_messages(&self) -> Vec<Message> {
        let branch = self.get_branch();
        let latest_compaction = branch.iter().enumerate().rev().find_map(|(idx, entry)| {
            let SessionEntry::Compaction {
                summary,
                first_kept_id,
                ..
            } = entry
            else {
                return None;
            };
            Some((idx, summary.as_str(), first_kept_id.as_str()))
        });

        let Some((_compaction_idx, summary, first_kept_id)) = latest_compaction else {
            return branch
                .into_iter()
                .filter_map(|entry| match entry {
                    SessionEntry::Message { message, .. } => Some(message.clone()),
                    _ => None,
                })
                .collect();
        };

        let mut active = Vec::new();
        let summary_text = summary.trim();
        if !summary_text.is_empty() {
            active.push(Message::user(summary_text.to_string()));
        }

        if first_kept_id.is_empty() {
            return active;
        }

        let mut keep = false;
        for entry in branch {
            if entry.id() == Some(first_kept_id) {
                keep = true;
            }
            if !keep {
                continue;
            }
            if let SessionEntry::Message { message, .. } = entry {
                active.push(message.clone());
            }
        }

        active
    }

    /// Get the active model-visible branch entries.
    ///
    /// This is a convenience wrapper over `get_active_messages()` for callers
    /// that still want borrowed-like iteration semantics at the message layer.
    pub fn active_message_count(&self) -> usize {
        self.get_active_messages().len()
    }

    /// Build the full tree structure from all entries.
    pub fn get_tree(&self) -> Vec<TreeNode> {
        // Separate roots (entries with no parent_id that have an id) and children
        let mut children_map: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut roots: Vec<usize> = Vec::new();

        for (i, entry) in self.entries.iter().enumerate() {
            match entry.parent_id() {
                Some(pid) => {
                    children_map.entry(pid).or_default().push(i);
                }
                None => {
                    roots.push(i);
                }
            }
        }

        roots
            .into_iter()
            .map(|i| self.build_subtree(i, &children_map))
            .collect()
    }

    fn build_subtree(&self, idx: usize, children_map: &HashMap<&str, Vec<usize>>) -> TreeNode {
        let entry = &self.entries[idx];
        let children = entry
            .id()
            .and_then(|id| children_map.get(id))
            .map(|child_indices| {
                child_indices
                    .iter()
                    .map(|&ci| self.build_subtree(ci, children_map))
                    .collect()
            })
            .unwrap_or_default();

        TreeNode {
            entry: entry.clone(),
            children,
        }
    }

    /// Change the current position in the tree to a different entry.
    pub fn navigate(&mut self, target_id: &str) -> Result<()> {
        let exists = self.entries.iter().any(|e| e.id() == Some(target_id));
        if !exists {
            return Err(crate::error::Error::Session(format!(
                "entry not found: {target_id}"
            )));
        }
        self.leaf_id = Some(target_id.to_string());
        Ok(())
    }

    /// Create a new session file containing only entries up to (and including) the
    /// given entry_id, following its branch from root.
    pub fn fork(&self, entry_id: &str, new_path: &Path) -> Result<SessionManager> {
        // Build the branch to this entry
        let id_map: HashMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.id().map(|id| (id, i)))
            .collect();

        let mut branch_indices = Vec::new();
        let mut current = Some(entry_id);

        while let Some(id) = current {
            if let Some(&idx) = id_map.get(id) {
                branch_indices.push(idx);
                current = self.entries[idx].parent_id();
            } else {
                break;
            }
        }

        branch_indices.reverse();

        // Collect header + branch entries
        let mut forked_entries = Vec::new();
        for entry in &self.entries {
            if matches!(entry, SessionEntry::Header { .. }) {
                forked_entries.push(entry.clone());
                break;
            }
        }
        for idx in &branch_indices {
            forked_entries.push(self.entries[*idx].clone());
        }

        // Also include any Label entries that reference entries in our branch
        let branch_ids: std::collections::HashSet<String> = forked_entries
            .iter()
            .filter_map(|e| e.id().map(String::from))
            .collect();
        let labels: Vec<SessionEntry> = self
            .entries
            .iter()
            .filter(|e| {
                matches!(e, SessionEntry::Label { entry_id, .. } if branch_ids.contains(entry_id.as_str()))
            })
            .cloned()
            .collect();
        forked_entries.extend(labels);

        // Also include session metadata so names/summaries survive forks.
        let meta_entries: Vec<SessionEntry> = self
            .entries
            .iter()
            .filter(|e| matches!(e, SessionEntry::SessionMeta { .. }))
            .cloned()
            .collect();
        forked_entries.extend(meta_entries);

        // Write to new file
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        {
            use std::io::Write;
            let mut file = std::fs::File::create(new_path)?;
            for entry in &forked_entries {
                let line = serde_json::to_string(entry)?;
                writeln!(file, "{line}")?;
            }
        }

        let leaf_id = forked_entries
            .iter()
            .rev()
            .find_map(|e| e.id())
            .map(String::from);

        Ok(SessionManager {
            entries: forked_entries,
            path: Some(new_path.to_path_buf()),
            leaf_id,
            session_name: self.session_name.clone(),
            session_summary: self.session_summary.clone(),
        })
    }

    /// Return all persisted recovery checkpoints in session order.
    pub fn recovery_checkpoints(&self) -> Vec<RecoveryCheckpoint> {
        self.entries
            .iter()
            .filter_map(|entry| {
                let SessionEntry::Custom {
                    custom_type, data, ..
                } = entry
                else {
                    return None;
                };
                if custom_type != RECOVERY_CHECKPOINT_CUSTOM_TYPE {
                    return None;
                }
                serde_json::from_value(data.clone()).ok()
            })
            .collect()
    }

    /// Build a recovery ledger from persisted recovery checkpoint entries.
    pub fn recovery_ledger(&self) -> crate::agent::RecoveryLedger {
        crate::agent::RecoveryLedger::from_checkpoints(self.recovery_checkpoints())
    }

    /// Get all entries.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Get the session file path.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Get the current leaf id.
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    /// Set the current leaf id for an in-memory session.
    pub fn set_leaf_id_for_in_memory(&mut self, leaf_id: String) {
        if self.path.is_none() {
            self.leaf_id = Some(leaf_id);
        }
    }

    pub fn snapshot_with_pending_user_message(
        &self,
        id: String,
        timestamp: u64,
        text: String,
    ) -> Self {
        let mut session = self.clone();
        let parent_id = session.leaf_id().map(str::to_string);
        session.entries.push(SessionEntry::Message {
            id: id.clone(),
            parent_id,
            message: Message::User(UserMessage {
                content: vec![ContentBlock::Text { text }],
                timestamp,
            }),
        });
        session.leaf_id = Some(id);
        session
    }

    /// Get the stable session id derived from the persisted file name, if any.
    pub fn session_id(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().to_string())
    }

    /// List available sessions in a directory.
    pub fn list(session_dir: &Path) -> Result<Vec<SessionInfo>> {
        Self::list_page(session_dir, 0, usize::MAX, None)
    }

    pub fn list_page(
        session_dir: &Path,
        offset: usize,
        limit: usize,
        query: Option<&str>,
    ) -> Result<Vec<SessionInfo>> {
        let mut files = recent_session_files(session_dir)?;
        if offset > 0 {
            files = files.into_iter().skip(offset).collect();
        }
        if query.is_none() {
            files.truncate(limit);
        }

        let query = query
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let mut sessions = Vec::new();
        for (path, updated_at) in files {
            if let Ok(info) = read_session_info(&path, updated_at) {
                if query
                    .as_deref()
                    .is_none_or(|needle| session_info_matches(&info, needle))
                {
                    sessions.push(info);
                    if query.is_none() && sessions.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(sessions)
    }
}

/// Sanitize a message history for API submission.
///
/// Strips unpaired tool_call blocks (assistant tool_use without matching tool_result)
/// and orphaned tool_result messages (tool_result without matching tool_use).
/// This handles both old sessions (before tool_result persistence) and corrupted
/// sessions where tool calls were partially recorded.
pub fn sanitize_messages(messages: &mut Vec<Message>) {
    use std::collections::HashSet;

    // Collect tool_result IDs to find which tool_calls have results
    let result_ids: HashSet<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(tr) => Some(tr.tool_call_id.clone()),
            _ => None,
        })
        .collect();

    // Strip unpaired tool_call blocks from assistant messages
    for msg in messages.iter_mut() {
        if let Message::Assistant(assistant) = msg {
            assistant.content.retain(|block| match block {
                imp_llm::ContentBlock::ToolCall { id, .. } => result_ids.contains(id),
                _ => true,
            });
        }
    }

    // Remove empty assistant messages left after stripping
    messages.retain(|msg| match msg {
        Message::Assistant(a) => !a.content.is_empty(),
        _ => true,
    });

    // Strip orphaned tool_results whose tool_call no longer exists
    let remaining_call_ids: HashSet<String> = messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.content.iter().filter_map(|b| match b {
                imp_llm::ContentBlock::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            })),
            _ => None,
        })
        .flatten()
        .collect();
    messages.retain(|msg| match msg {
        Message::ToolResult(tr) => remaining_call_ids.contains(&tr.tool_call_id),
        _ => true,
    });

    // Reorder: ensure each tool_result follows the assistant message that
    // contains its tool_call. Session persistence can write tool_results
    // before the assistant message (ToolExecutionEnd fires before TurnEnd).
    reorder_tool_results(messages);
}

/// Move tool_result messages so they immediately follow the assistant
/// message containing the matching tool_call.
fn reorder_tool_results(messages: &mut Vec<Message>) {
    use std::collections::HashMap;

    // Build map: tool_call_id → index of the assistant message that has it
    let mut call_to_assistant: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if let Message::Assistant(a) = msg {
            for block in &a.content {
                if let imp_llm::ContentBlock::ToolCall { id, .. } = block {
                    call_to_assistant.insert(id.clone(), i);
                }
            }
        }
    }

    // Separate tool_results that are out of order
    let mut deferred: Vec<(usize, Message)> = Vec::new(); // (target_after_idx, msg)
    let mut i = 0;
    while i < messages.len() {
        if let Message::ToolResult(tr) = &messages[i] {
            if let Some(&assistant_idx) = call_to_assistant.get(&tr.tool_call_id) {
                if i < assistant_idx {
                    // tool_result appears before its assistant — pull it out
                    let msg = messages.remove(i);
                    deferred.push((assistant_idx, msg));
                    // Adjust assistant indices after removal
                    for v in call_to_assistant.values_mut() {
                        if *v > i {
                            *v -= 1;
                        }
                    }
                    for d in &mut deferred {
                        if d.0 > i {
                            d.0 -= 1;
                        }
                    }
                    continue; // don't increment i
                }
            }
        }
        i += 1;
    }

    // Re-insert deferred tool_results after their assistant messages
    // Sort by target index descending so insertions don't shift earlier targets
    deferred.sort_by(|a, b| b.0.cmp(&a.0));
    for (target_idx, msg) in deferred {
        let insert_at = (target_idx + 1).min(messages.len());
        messages.insert(insert_at, msg);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_ledger_tests;
