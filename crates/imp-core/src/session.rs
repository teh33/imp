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

/// Extract the first text content from a message.
fn extract_text(message: &Message) -> Option<String> {
    let blocks = match message {
        Message::User(u) => &u.content,
        Message::Assistant(a) => &a.content,
        Message::ToolResult(t) => &t.content,
    };
    blocks.iter().find_map(|b| match b {
        imp_llm::ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    })
}

fn derive_session_summary(entries: &[SessionEntry]) -> Option<String> {
    let mut parts = Vec::new();

    for entry in entries.iter().rev() {
        match entry {
            SessionEntry::SessionMeta {
                summary: Some(summary),
                ..
            } if !summary.trim().is_empty() => {
                return Some(truncate_chars_with_suffix(summary.trim(), 120, "…"));
            }
            // Session summaries are stored in compact session-meta entries.
            SessionEntry::Compaction { summary, .. } => {
                let trimmed = cleanup_summary_text(summary);
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            SessionEntry::Message { message, .. } => {
                if let Message::Assistant(_) = message {
                    if let Some(text) = extract_text(message) {
                        let trimmed = cleanup_summary_text(&text);
                        if !trimmed.is_empty() {
                            parts.push(trimmed);
                        }
                    }
                }
            }
            _ => {}
        }

        if parts.len() >= 3 {
            break;
        }
    }

    if parts.is_empty() {
        return None;
    }

    let joined = parts.into_iter().rev().collect::<Vec<_>>().join(" ");
    let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(truncate_chars_with_suffix(&collapsed, 120, "…"))
    }
}

fn cleanup_summary_text(text: &str) -> String {
    let mut collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    for prefix in [
        "summary:",
        "session summary:",
        "assistant summary:",
        "in summary,",
        "to summarize,",
    ] {
        if collapsed.to_ascii_lowercase().starts_with(prefix) {
            collapsed = collapsed[prefix.len()..].trim().to_string();
            break;
        }
    }

    collapsed
}

fn preferred_title_candidate(
    first_prompt: Option<&str>,
    summary: Option<&str>,
    max_chars: usize,
) -> Option<String> {
    let first_prompt = first_prompt
        .map(cleanup_summary_text)
        .filter(|text| !text.is_empty());
    let summary = summary
        .map(cleanup_summary_text)
        .filter(|text| !text.is_empty());

    match (first_prompt.as_deref(), summary.as_deref()) {
        (Some(prompt), Some(summary)) => {
            let prompt_title = literal_topic_title(prompt, max_chars);
            let summary_title = literal_topic_title(summary, max_chars);
            choose_better_title(prompt_title, summary_title, max_chars)
        }
        (Some(prompt), None) => literal_topic_title(prompt, max_chars),
        (None, Some(summary)) => literal_topic_title(summary, max_chars),
        (None, None) => None,
    }
}

fn choose_better_title(
    prompt_title: Option<String>,
    summary_title: Option<String>,
    max_chars: usize,
) -> Option<String> {
    match (prompt_title, summary_title) {
        (Some(prompt), Some(summary)) => {
            if is_generic_title(&prompt) && !is_generic_title(&summary) {
                Some(summary)
            } else if !is_generic_title(&prompt) && is_generic_title(&summary) {
                Some(prompt)
            } else if topic_word_count(&summary) > topic_word_count(&prompt) {
                Some(summary)
            } else {
                Some(truncate_chars_with_suffix(&prompt, max_chars, "…"))
            }
        }
        (Some(prompt), None) => Some(prompt),
        (None, Some(summary)) => Some(summary),
        (None, None) => None,
    }
}

fn topic_word_count(title: &str) -> usize {
    title
        .split_whitespace()
        .filter(|word| word.len() >= 4)
        .count()
}

fn literal_topic_title(text: &str, max_chars: usize) -> Option<String> {
    let cleaned = cleanup_summary_text(text);
    if cleaned.is_empty() {
        return None;
    }

    let literal = concise_topic_phrase(&cleaned, max_chars);
    if !literal.trim().is_empty() && !is_generic_title(&literal) {
        return Some(literal);
    }

    let heuristic = summarize_session_title(&cleaned, max_chars);
    if !heuristic.trim().is_empty() && !is_generic_title(&heuristic) {
        return Some(heuristic);
    }

    Some(truncate_chars_with_suffix(cleaned.trim(), max_chars, "…"))
}

fn is_generic_title(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }

    let generic_words = [
        "yes", "yeah", "yep", "ok", "okay", "sure", "think", "some", "pretty", "good", "great",
        "nice", "maybe", "just", "really", "thing", "stuff",
    ];

    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() <= 2 && words.iter().all(|w| generic_words.contains(w)) {
        return true;
    }

    words.iter().filter(|w| generic_words.contains(w)).count() >= words.len().saturating_sub(1)
}

fn concise_topic_phrase(text: &str, max_chars: usize) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    let mut phrase = collapsed
        .split_terminator(['.', '!', '?', ';', ':'])
        .find_map(|part| {
            let trimmed = part.trim();
            if trimmed.split_whitespace().count() >= 3 {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .unwrap_or(collapsed);

    let leading_phrases = [
        "we should ",
        "let's ",
        "i want to ",
        "i'd like to ",
        "can we ",
        "can you ",
        "could we ",
        "could you ",
        "would you ",
        "please ",
        "help me ",
        "yes ",
        "yeah ",
        "ok ",
        "okay ",
        "sure ",
        "i think ",
        "think ",
    ];

    let lower = phrase.to_ascii_lowercase();
    for prefix in leading_phrases {
        if let Some(stripped) = lower.strip_prefix(prefix) {
            phrase = stripped.trim().to_string();
            break;
        }
    }

    let stopwords = [
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "but",
        "by",
        "for",
        "from",
        "how",
        "i",
        "if",
        "in",
        "into",
        "is",
        "it",
        "its",
        "me",
        "my",
        "of",
        "on",
        "or",
        "please",
        "so",
        "that",
        "the",
        "their",
        "them",
        "there",
        "these",
        "they",
        "this",
        "to",
        "up",
        "we",
        "what",
        "when",
        "where",
        "which",
        "while",
        "with",
        "would",
        "can",
        "could",
        "should",
        "work",
        "working",
        "improving",
        "improve",
        "usability",
        "currently",
        "displayed",
        "shown",
        "information",
        "some",
        "pretty",
        "really",
        "just",
        "think",
        "yes",
        "yeah",
        "okay",
        "ok",
        "sure",
    ];

    let normalized = phrase
        .replace("/resume", "resume")
        .replace("chat summaries", "chat_summaries")
        .replace("top bar", "top_bar")
        .replace("session picker", "session_picker")
        .replace("oauth login", "oauth_login")
        .replace("provider refresh", "provider_refresh");

    let mut tokens = Vec::new();
    for raw in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if stopwords.contains(&lower.as_str()) {
            continue;
        }
        if tokens.iter().any(|existing: &String| existing == &lower) {
            continue;
        }
        tokens.push(lower);
    }

    if tokens.is_empty() {
        let words: Vec<&str> = phrase.split_whitespace().collect();
        let take = words.len().min(4);
        return truncate_chars_with_suffix(&words[..take].join(" "), max_chars, "…");
    }

    let mut out = tokens
        .into_iter()
        .take(5)
        .map(|token| match token.as_str() {
            "chat_summaries" => "chat summaries".to_string(),
            "top_bar" => "top bar".to_string(),
            "session_picker" => "session picker".to_string(),
            "oauth_login" => "oauth login".to_string(),
            "provider_refresh" => "provider refresh".to_string(),
            _ => token,
        })
        .collect::<Vec<_>>();

    if out.len() > 4 {
        out.truncate(4);
    }

    let mut out = out.join(" ");

    out = out.replace("resume chat summaries", "resume + summaries");
    truncate_chars_with_suffix(out.trim(), max_chars, "…")
}

fn summarize_session_title(text: &str, max_chars: usize) -> String {
    let collapsed = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut normalized = collapsed.to_ascii_lowercase();

    for prefix in [
        "can we ",
        "could we ",
        "can you ",
        "could you ",
        "would you ",
        "please ",
        "please can you ",
        "please could you ",
        "help me ",
        "i want to ",
        "i'd like to ",
        "let's ",
    ] {
        if let Some(stripped) = normalized.strip_prefix(prefix) {
            normalized = stripped.to_string();
            break;
        }
    }

    for (phrase, token) in [
        ("top bar", "top_bar"),
        ("prompt box", "prompt_box"),
        ("thinking level", "thinking_level"),
        ("model name", "model_name"),
        ("session name", "session_name"),
        ("chat title", "chat_title"),
        ("chat name", "chat_name"),
        ("session id", "session_id"),
        ("context window", "context_window"),
    ] {
        normalized = normalized.replace(phrase, token);
    }

    let mentions_top_bar_layout = normalized.contains("top_bar")
        && (normalized.contains("display")
            || normalized.contains("displayed")
            || normalized.contains("shown")
            || normalized.contains("information"));

    let verbs = [
        "fix",
        "adjust",
        "update",
        "change",
        "move",
        "rename",
        "remove",
        "add",
        "show",
        "hide",
        "improve",
        "refactor",
        "debug",
        "investigate",
        "implement",
        "summarize",
    ];

    let stopwords = [
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "but",
        "by",
        "for",
        "from",
        "get",
        "have",
        "how",
        "i",
        "if",
        "in",
        "instead",
        "into",
        "is",
        "it",
        "its",
        "me",
        "my",
        "now",
        "of",
        "on",
        "or",
        "please",
        "right",
        "so",
        "string",
        "that",
        "the",
        "their",
        "them",
        "then",
        "there",
        "these",
        "they",
        "this",
        "to",
        "up",
        "we",
        "what",
        "when",
        "where",
        "which",
        "while",
        "with",
        "would",
        "listed",
        "resume",
        "prompt",
        "first",
        "summarized",
        "summarize",
        "information",
        "display",
        "displayed",
        "shown",
        "currently",
    ];

    let mut verb: Option<String> = None;
    let mut nouns: Vec<String> = Vec::new();

    for raw in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        if verb.is_none() && verbs.contains(&raw) {
            verb = Some(raw.to_string());
            continue;
        }
        if stopwords.contains(&raw) {
            continue;
        }
        if nouns.iter().any(|existing| existing == raw) {
            continue;
        }
        nouns.push(raw.to_string());
    }

    let mut parts = Vec::new();
    if let Some(verb) = verb {
        parts.push(verb);
    }

    for noun in nouns {
        if parts.len() >= 4 {
            break;
        }
        parts.push(noun.clone());
        if noun == "top_bar" && mentions_top_bar_layout && parts.len() < 4 {
            parts.push("layout".to_string());
        }
    }

    if parts.is_empty() {
        parts.push(collapsed.trim().to_string());
    }

    let summary = parts
        .into_iter()
        .map(|part| match part.as_str() {
            "top_bar" => "top bar".to_string(),
            "prompt_box" => "prompt box".to_string(),
            "thinking_level" => "thinking level".to_string(),
            "model_name" => "model name".to_string(),
            "session_name" => "session name".to_string(),
            "chat_title" => "chat title".to_string(),
            "chat_name" => "chat name".to_string(),
            "session_id" => "session id".to_string(),
            "context_window" => "context window".to_string(),
            _ => part,
        })
        .collect::<Vec<_>>()
        .join(" ");

    truncate_chars_with_suffix(summary.trim(), max_chars, "…")
}

fn recent_session_files(session_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    for dir_entry in std::fs::read_dir(session_dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }

        let modified = dir_entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        let updated_at = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        files.push((path, updated_at, modified));
    }

    files.sort_by(|(path_a, _, modified_a), (path_b, _, modified_b)| {
        modified_b.cmp(modified_a).then_with(|| path_b.cmp(path_a))
    });
    Ok(files
        .into_iter()
        .map(|(path, updated_at, _)| (path, updated_at))
        .collect())
}

fn session_info_matches(info: &SessionInfo, needle: &str) -> bool {
    info.name
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info
            .summary
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info
            .first_message
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info.cwd.to_ascii_lowercase().contains(needle)
        || info.id.to_ascii_lowercase().contains(needle)
}

fn read_session_info(path: &Path, updated_at: u64) -> Result<SessionInfo> {
    use std::io::BufRead;

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut cwd = String::new();
    let mut created_at = 0;
    let mut message_count = 0;
    let mut compaction_count = 0;
    let mut first_message = None;
    let mut last_message = None;
    let mut name = None;
    let mut summary = None;
    let mut summary_parts = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<SessionEntry>(&line) else {
            continue;
        };
        match entry {
            SessionEntry::Header {
                cwd: entry_cwd,
                created_at: entry_created_at,
                ..
            } => {
                cwd = entry_cwd;
                created_at = entry_created_at;
            }
            SessionEntry::Message { message, .. } => {
                message_count += 1;
                if first_message.is_none() {
                    first_message = extract_text(&message);
                }
                if let Some(text) = extract_text(&message) {
                    last_message = Some(text);
                }
                if summary.is_none() && summary_parts.len() < 3 {
                    if let Message::Assistant(_) = message {
                        if let Some(text) = extract_text(&message) {
                            let trimmed = cleanup_summary_text(&text);
                            if !trimmed.is_empty() {
                                summary_parts.push(trimmed);
                            }
                        }
                    }
                }
            }
            SessionEntry::Compaction {
                summary: compaction_summary,
                ..
            } => {
                compaction_count += 1;
                if summary.is_none() {
                    let trimmed = cleanup_summary_text(&compaction_summary);
                    if !trimmed.is_empty() {
                        summary_parts.clear();
                        summary_parts.push(trimmed);
                    }
                }
            }
            SessionEntry::SessionMeta {
                name: meta_name,
                summary: meta_summary,
                ..
            } => {
                name = meta_name;
                if meta_summary
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    summary = meta_summary
                        .map(|value| truncate_chars_with_suffix(value.trim(), 120, "…"));
                }
            }
            _ => {}
        }
    }

    let summary = summary.or_else(|| {
        if summary_parts.is_empty() {
            None
        } else {
            let collapsed = summary_parts
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if collapsed.is_empty() {
                None
            } else {
                Some(truncate_chars_with_suffix(&collapsed, 120, "…"))
            }
        }
    });

    Ok(SessionInfo {
        id: path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.to_path_buf(),
        cwd,
        created_at,
        updated_at,
        message_count: if message_count == 0 {
            compaction_count
        } else {
            message_count
        },
        first_message,
        last_message,
        name,
        summary,
    })
}

/// Read just the first non-empty line of a file.
fn read_first_line(path: &Path) -> Result<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            return Ok(line);
        }
    }
    Err(crate::error::Error::Session("empty file".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use imp_llm::{
        auth::{ApiKey, AuthStore},
        model::{Capabilities, ModelMeta, ModelPricing},
        provider::{Context, Provider, RequestOptions},
        AssistantMessage, ContentBlock, Message, StopReason, StreamEvent,
    };
    use tempfile::TempDir;

    struct NoopProvider {
        models: Vec<ModelMeta>,
    }

    #[async_trait]
    impl Provider for NoopProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> std::pin::Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>
        {
            Box::pin(stream::empty())
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok(String::new())
        }

        fn id(&self) -> &str {
            "noop"
        }

        fn models(&self) -> &[ModelMeta] {
            &self.models
        }
    }

    fn make_test_model() -> Model {
        Model {
            meta: ModelMeta {
                id: "test-model".into(),
                provider: "test-provider".into(),
                name: "Test Model".into(),
                context_window: 8192,
                max_output_tokens: 2048,
                pricing: ModelPricing {
                    input_per_mtok: 1.0,
                    output_per_mtok: 2.0,
                    cache_read_per_mtok: 0.5,
                    cache_write_per_mtok: 1.0,
                },
                capabilities: Capabilities {
                    reasoning: false,
                    images: false,
                    tool_use: true,
                },
            },
            provider: std::sync::Arc::new(NoopProvider { models: Vec::new() }),
        }
    }

    fn make_msg_entry(id: &str, text: &str) -> SessionEntry {
        SessionEntry::Message {
            id: id.to_string(),
            parent_id: None, // append() will set this
            message: Message::user(text),
        }
    }

    #[test]
    fn summarized_title_compacts_request_into_short_label() {
        let title = summarize_session_title(
            "can we adjust the information that is displayed in the top bar",
            48,
        );
        assert_eq!(title, "adjust top bar layout");
    }

    #[test]
    fn literal_topic_title_prefers_subject_words_over_compaction() {
        let title = literal_topic_title(
            "can we work on improving the usability of /resume and the chat summaries?",
            64,
        )
        .unwrap();

        assert!(title.contains("resume") || title.contains("summaries"));
        assert!(title.split_whitespace().count() <= 5);
    }

    #[test]
    fn generic_summary_title_falls_back_to_more_descriptive_phrase() {
        let title = literal_topic_title(
            "yes think some pretty significant issues with oauth login persistence and provider refresh",
            64,
        )
        .unwrap();

        assert!(title.contains("oauth") || title.contains("login"));
        assert!(title.split_whitespace().count() <= 5);
        assert_ne!(title, "yes think some pretty");
    }

    #[test]
    fn session_titles_can_be_derived_from_summary_text() {
        let info = SessionInfo {
            id: "abc".into(),
            path: PathBuf::from("/tmp/abc.jsonl"),
            cwd: "/tmp/project".into(),
            created_at: 0,
            updated_at: 0,
            message_count: 1,
            first_message: Some("help me with oauth login issues".into()),
            last_message: Some("fixed oauth login issues".into()),
            name: None,
            summary: Some(
                "Investigated OAuth login failures and refreshed provider auth flow".into(),
            ),
        };

        let title = info.title(48).unwrap();
        assert!(!title.is_empty());
        assert!(title.contains("oauth") || title.contains("login") || title.contains("provider"));
        assert!(title.split_whitespace().count() <= 5);
    }

    #[test]
    fn session_compaction_active_messages_replace_prefix_with_summary() {
        let mut mgr = SessionManager::in_memory();

        mgr.append(make_msg_entry("u1", "first request")).unwrap();
        mgr.append(SessionEntry::Message {
            id: "a1".into(),
            parent_id: None,
            message: Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "initial answer".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            }),
        })
        .unwrap();
        mgr.append(make_msg_entry("u2", "latest request")).unwrap();
        mgr.append(SessionEntry::Compaction {
            id: "c1".into(),
            parent_id: None,
            summary: "Compaction summary of earlier work".into(),
            first_kept_id: "u2".into(),
            tokens_before: 100,
            tokens_after: 40,
        })
        .unwrap();
        mgr.append(SessionEntry::Message {
            id: "a2".into(),
            parent_id: None,
            message: Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "follow-up answer".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 2,
            }),
        })
        .unwrap();

        let raw = mgr.get_messages();
        assert_eq!(raw.len(), 4);

        let active = mgr.get_active_messages();
        assert_eq!(active.len(), 3);
        match &active[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => {
                    assert_eq!(text, "Compaction summary of earlier work")
                }
                other => panic!("unexpected summary content: {other:?}"),
            },
            other => panic!("unexpected active message: {other:?}"),
        }
        match &active[1] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => assert_eq!(text, "latest request"),
                other => panic!("unexpected kept user content: {other:?}"),
            },
            other => panic!("unexpected kept message: {other:?}"),
        }
    }

    #[test]
    fn session_compaction_active_messages_fall_back_to_raw_when_first_kept_missing() {
        let mut mgr = SessionManager::in_memory();
        mgr.append(make_msg_entry("u1", "hello")).unwrap();
        mgr.append(SessionEntry::Compaction {
            id: "c1".into(),
            parent_id: None,
            summary: "summary only".into(),
            first_kept_id: "missing".into(),
            tokens_before: 10,
            tokens_after: 3,
        })
        .unwrap();

        let active = mgr.get_active_messages();
        assert_eq!(active.len(), 1);
        match &active[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => assert_eq!(text, "summary only"),
                other => panic!("unexpected summary-only content: {other:?}"),
            },
            other => panic!("unexpected active message: {other:?}"),
        }
    }

    #[test]
    fn session_compaction_fork_preserves_compacted_branch_semantics() {
        let tmp = TempDir::new().unwrap();
        let fork_path = tmp.path().join("forked.jsonl");

        let mut mgr = SessionManager::in_memory();
        mgr.append(make_msg_entry("u1", "older")).unwrap();
        mgr.append(make_msg_entry("u2", "newer")).unwrap();
        mgr.append(SessionEntry::Compaction {
            id: "c1".into(),
            parent_id: None,
            summary: "summary older".into(),
            first_kept_id: "u2".into(),
            tokens_before: 20,
            tokens_after: 8,
        })
        .unwrap();
        mgr.append(SessionEntry::Message {
            id: "a2".into(),
            parent_id: None,
            message: Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 3,
            }),
        })
        .unwrap();

        let forked = mgr.fork("a2", &fork_path).unwrap();
        let active = forked.get_active_messages();
        assert_eq!(active.len(), 3);
        match &active[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => assert_eq!(text, "summary older"),
                other => panic!("unexpected summary content: {other:?}"),
            },
            other => panic!("unexpected active message: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_jsonl_is_private_on_disk() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new(Path::new("/tmp"), tmp.path()).unwrap();
        mgr.append_tool_result_message(ToolResultMessage {
            tool_call_id: "call-private".into(),
            tool_name: "bash".into(),
            content: vec![ContentBlock::Text {
                text: "private prompt".into(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: 0,
        })
        .unwrap();

        let mode = std::fs::metadata(mgr.path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn append_tool_result_message_persists_redacted_content() {
        let tmp = tempfile::tempdir().unwrap();
        let mut mgr = SessionManager::new(Path::new("/tmp"), tmp.path()).unwrap();
        mgr.append_tool_result_message(ToolResultMessage {
            tool_call_id: "call-redacted".into(),
            tool_name: "bash".into(),
            content: vec![ContentBlock::Text {
                text: "[REDACTED:test-service]".into(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: 0,
        })
        .unwrap();

        let path = mgr.path().unwrap();
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(contents.contains("[REDACTED:test-service]"));
        assert!(!contents.contains("native-secret-value"));
    }

    #[test]
    fn session_create_append_reopen() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        mgr.append(make_msg_entry("m1", "hello")).unwrap();
        mgr.append(make_msg_entry("m2", "world")).unwrap();
        mgr.append(make_msg_entry("m3", "!")).unwrap();

        let path = mgr.path().unwrap().to_path_buf();
        assert!(path.exists());

        // Reopen and verify messages match
        let reopened = SessionManager::open(&path).unwrap();
        let original_msgs = mgr.get_messages();
        let reopened_msgs = reopened.get_messages();
        assert_eq!(original_msgs.len(), reopened_msgs.len());
        assert_eq!(reopened_msgs.len(), 3);

        // Verify parent chain: m1 has no parent, m2's parent is m1, m3's parent is m2
        let entries = reopened.entries();
        for entry in entries {
            if let SessionEntry::Message { id, parent_id, .. } = entry {
                match id.as_str() {
                    "m1" => assert_eq!(*parent_id, None),
                    "m2" => assert_eq!(parent_id.as_deref(), Some("m1")),
                    "m3" => assert_eq!(parent_id.as_deref(), Some("m2")),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn session_branch() {
        let mut mgr = SessionManager::in_memory();
        // Append 5 messages (m1..m5)
        for i in 1..=5 {
            mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
                .unwrap();
        }
        assert_eq!(mgr.get_messages().len(), 5);
        assert_eq!(mgr.leaf_id(), Some("m5"));

        // Navigate back to m3
        mgr.navigate("m3").unwrap();
        assert_eq!(mgr.leaf_id(), Some("m3"));

        // Append 2 new messages on the branch
        mgr.append(make_msg_entry("b1", "branch 1")).unwrap();
        mgr.append(make_msg_entry("b2", "branch 2")).unwrap();

        // get_branch should return: header-less chain of m1, m2, m3, b1, b2
        let branch = mgr.get_branch();
        let branch_ids: Vec<Option<&str>> = branch.iter().map(|e| e.id()).collect();
        assert_eq!(
            branch_ids,
            vec![Some("m1"), Some("m2"), Some("m3"), Some("b1"), Some("b2")]
        );
        assert_eq!(mgr.get_messages().len(), 5);

        // Navigate back to m5 to verify original branch still works
        mgr.navigate("m5").unwrap();
        let main_branch = mgr.get_branch();
        let main_ids: Vec<Option<&str>> = main_branch.iter().map(|e| e.id()).collect();
        assert_eq!(
            main_ids,
            vec![Some("m1"), Some("m2"), Some("m3"), Some("m4"), Some("m5")]
        );
    }

    #[test]
    fn session_fork() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        for i in 1..=5 {
            mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
                .unwrap();
        }

        let fork_path = session_dir.join("forked.jsonl");
        let forked = mgr.fork("m3", &fork_path).unwrap();

        // Forked session should have header + m1, m2, m3
        assert_eq!(forked.get_messages().len(), 3);
        assert_eq!(forked.leaf_id(), Some("m3"));
        assert!(fork_path.exists());

        // Reopen the forked file and verify
        let reopened = SessionManager::open(&fork_path).unwrap();
        assert_eq!(reopened.get_messages().len(), 3);
    }

    #[test]
    fn session_list() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        // Create two sessions
        let mut s1 = SessionManager::new(&cwd, &session_dir).unwrap();
        s1.append(make_msg_entry("a1", "first session")).unwrap();
        s1.set_name("First");

        let mut s2 = SessionManager::new(&cwd, &session_dir).unwrap();
        s2.append(make_msg_entry("b1", "second session")).unwrap();
        s2.append(make_msg_entry("b2", "more stuff")).unwrap();
        s2.set_summary("Second session summary");

        let sessions = SessionManager::list(&session_dir).unwrap();
        assert_eq!(sessions.len(), 2);

        // Both should have the right cwd
        for s in &sessions {
            assert_eq!(s.cwd, cwd.to_string_lossy().to_string());
        }

        // One has 1 message, the other has 2
        let mut counts: Vec<usize> = sessions.iter().map(|s| s.message_count).collect();
        counts.sort();
        assert_eq!(counts, vec![1, 2]);

        // first_message and last_message should be set
        for s in &sessions {
            assert!(s.first_message.is_some());
            assert!(s.last_message.is_some());
        }

        assert!(sessions.iter().any(|s| s.name.as_deref() == Some("First")));
        assert!(sessions
            .iter()
            .any(|s| s.summary.as_deref() == Some("Second session summary")));
    }

    #[test]
    fn session_list_page_orders_by_updated_time_and_limits() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        let mut old = SessionManager::new(&cwd, &session_dir).unwrap();
        old.append(make_msg_entry("old", "old session")).unwrap();
        let _old_path = old.path().unwrap().to_path_buf();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut new = SessionManager::new(&cwd, &session_dir).unwrap();
        new.append(make_msg_entry("new", "new session")).unwrap();
        let new_path = new.path().unwrap().to_path_buf();

        let sessions = SessionManager::list_page(&session_dir, 0, 1, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].path, new_path);
    }

    #[test]
    fn session_list_captures_last_message() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        let mut session = SessionManager::new(&cwd, &session_dir).unwrap();
        session
            .append(make_msg_entry("first", "first prompt"))
            .unwrap();
        session
            .append(make_msg_entry("last", "latest response"))
            .unwrap();

        let sessions = SessionManager::list(&session_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].first_message.as_deref(), Some("first prompt"));
        assert_eq!(sessions[0].last_message.as_deref(), Some("latest response"));
    }

    #[test]
    fn session_list_includes_header_only_sessions() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        let mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        let id = mgr.session_id().unwrap();

        let sessions = SessionManager::list(&session_dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].message_count, 0);
        assert!(sessions[0].first_message.is_none());
    }

    #[test]
    fn session_continue_recent() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd_a = tmp.path().join("project-a");
        let cwd_b = tmp.path().join("project-b");

        // Create a session for cwd_a
        let mut s1 = SessionManager::new(&cwd_a, &session_dir).unwrap();
        s1.append(make_msg_entry("a1", "hello from a")).unwrap();

        // Create a session for cwd_b
        let mut s2 = SessionManager::new(&cwd_b, &session_dir).unwrap();
        s2.append(make_msg_entry("b1", "hello from b")).unwrap();

        // continue_recent for cwd_a should find s1
        let continued = SessionManager::continue_recent(&cwd_a, &session_dir)
            .unwrap()
            .expect("should find a session");
        assert_eq!(continued.get_messages().len(), 1);

        // continue_recent for a non-existent cwd returns None
        let none =
            SessionManager::continue_recent(Path::new("/nonexistent"), &session_dir).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn session_name_and_summary_persist_across_reopen() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");

        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        mgr.append(make_msg_entry("m1", "hello world")).unwrap();
        mgr.set_name("Debug auth");
        mgr.set_summary("Investigating OAuth login failures");

        let path = mgr.path().unwrap().to_path_buf();
        let reopened = SessionManager::open(&path).unwrap();
        assert_eq!(reopened.name(), Some("Debug auth"));
        assert_eq!(
            reopened.summary(),
            Some("Investigating OAuth login failures")
        );
    }

    #[test]
    fn session_in_memory() {
        let mut mgr = SessionManager::in_memory();
        assert!(mgr.path().is_none());

        mgr.append(make_msg_entry("m1", "hello")).unwrap();
        mgr.append(make_msg_entry("m2", "world")).unwrap();

        assert_eq!(mgr.get_messages().len(), 2);
        assert_eq!(mgr.entries().len(), 2);
    }

    #[test]
    fn session_malformed_jsonl() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.jsonl");

        // Write a file with a mix of valid and invalid lines
        let content = format!(
            "{}\n\
             NOT VALID JSON\n\
             {}\n\
             {{\"type\":\"unknown_variant\",\"foo\":1}}\n\
             {}\n",
            serde_json::to_string(&SessionEntry::Header {
                version: 1,
                created_at: 1000,
                cwd: "/tmp".into(),
            })
            .unwrap(),
            serde_json::to_string(&SessionEntry::Message {
                id: "m1".into(),
                parent_id: None,
                message: Message::user("hello"),
            })
            .unwrap(),
            serde_json::to_string(&SessionEntry::Message {
                id: "m2".into(),
                parent_id: Some("m1".into()),
                message: Message::user("world"),
            })
            .unwrap(),
        );
        std::fs::write(&path, content).unwrap();

        // Should succeed, skipping the bad lines
        let mgr = SessionManager::open(&path).unwrap();
        // Header + 2 valid messages (bad lines skipped)
        assert_eq!(mgr.entries().len(), 3);
        assert_eq!(mgr.get_messages().len(), 2);
    }

    #[test]
    fn session_get_tree() {
        let mut mgr = SessionManager::in_memory();
        for i in 1..=3 {
            mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
                .unwrap();
        }
        // Branch from m2
        mgr.navigate("m2").unwrap();
        mgr.append(make_msg_entry("b1", "branch")).unwrap();

        let tree = mgr.get_tree();
        // Root should be m1 (no parent)
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].entry.id(), Some("m1"));

        // m1 -> m2
        assert_eq!(tree[0].children.len(), 1);
        let m2_node = &tree[0].children[0];
        assert_eq!(m2_node.entry.id(), Some("m2"));

        // m2 has two children: m3 and b1
        assert_eq!(m2_node.children.len(), 2);
        let child_ids: Vec<Option<&str>> = m2_node.children.iter().map(|n| n.entry.id()).collect();
        assert!(child_ids.contains(&Some("m3")));
        assert!(child_ids.contains(&Some("b1")));
    }

    #[test]
    fn append_assistant_turn_persists_canonical_usage_once() {
        let tmp = TempDir::new().unwrap();
        let session_dir = tmp.path().join("sessions");
        let cwd = tmp.path().join("project");
        let model = make_test_model();

        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        let message = AssistantMessage {
            content: vec![imp_llm::ContentBlock::Text {
                text: "done".into(),
            }],
            usage: Some(imp_llm::Usage {
                input_tokens: 100,
                output_tokens: 25,
                cache_read_tokens: 10,
                cache_write_tokens: 5,
            }),
            stop_reason: imp_llm::StopReason::EndTurn,
            timestamp: 123,
        };

        let (_assistant_id, usage_id) = mgr
            .append_assistant_turn(&model, 3, message.clone())
            .unwrap();
        assert!(usage_id.is_some());

        let (_assistant_id_2, usage_id_2) = mgr
            .append_assistant_turn(
                &model,
                4,
                AssistantMessage {
                    usage: None,
                    ..message
                },
            )
            .unwrap();
        assert!(usage_id_2.is_none());

        let usage_records = mgr.usage_records();
        assert_eq!(usage_records.len(), 1);
        assert_eq!(usage_records[0].turn_index, Some(3));
        assert_eq!(usage_records[0].provider.as_deref(), Some("test-provider"));
        assert_eq!(usage_records[0].model.as_deref(), Some("test-model"));
    }

    #[test]
    fn append_checkpoint_record_round_trips_and_lookup_works() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let session_dir = tmp.path().join("sessions");
        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();

        let record = SessionCheckpointRecord {
            version: CHECKPOINT_RECORD_VERSION,
            checkpoint_id: "cp-1".into(),
            created_at: 123,
            label: Some("before edits".into()),
            files: vec!["src/main.rs".into(), "src/lib.rs".into()],
        };
        mgr.append_checkpoint_record(record.clone()).unwrap();

        assert_eq!(mgr.checkpoint_records(), vec![record.clone()]);
        assert_eq!(
            mgr.find_checkpoint_record("cp-1").unwrap().label.as_deref(),
            Some("before edits")
        );
        assert_eq!(
            mgr.find_checkpoint_record("before edits")
                .unwrap()
                .checkpoint_id,
            "cp-1"
        );
    }

    #[test]
    fn restore_checkpoint_uses_checkpoint_state() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let session_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&cwd).unwrap();
        let file = cwd.join("main.rs");
        std::fs::write(&file, "original").unwrap();

        let checkpoint_state = crate::tools::CheckpointState::new();
        let checkpoint = checkpoint_state
            .snapshot_paths(std::slice::from_ref(&file), Some("before edits".into()))
            .unwrap()
            .unwrap();
        std::fs::write(&file, "modified").unwrap();

        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
        mgr.append_checkpoint_record(SessionCheckpointRecord {
            version: CHECKPOINT_RECORD_VERSION,
            checkpoint_id: checkpoint.id.clone(),
            created_at: checkpoint.created_at,
            label: checkpoint.label.clone(),
            files: checkpoint
                .files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
        })
        .unwrap();

        let restored = mgr
            .restore_checkpoint(&checkpoint_state, "before edits")
            .unwrap();
        assert_eq!(restored, vec![file.clone()]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
    }

    #[test]
    fn session_navigate_invalid() {
        let mut mgr = SessionManager::in_memory();
        mgr.append(make_msg_entry("m1", "hello")).unwrap();

        let result = mgr.navigate("nonexistent");
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod recovery_ledger_tests {
    use super::*;
    use crate::agent::{RecoveryCheckpoint, RecoveryCheckpointKind};

    #[test]
    fn recovery_checkpoint_entry_redacts_tool_args_and_output() {
        let raw_secret_arg = "super-secret-token";
        let raw_tool_output = "sensitive command output";
        let checkpoint = RecoveryCheckpoint {
            version: 1,
            turn: 3,
            kind: RecoveryCheckpointKind::ToolExecutionStart,
            tool_call_id: Some("call_123".to_string()),
            tool_name: Some("bash".to_string()),
            args_hash: Some("0123456789abcdef".to_string()),
            success: None,
            error_class: None,
            timestamp: 42,
        };

        let entry = recovery_checkpoint_entry("recovery-1", checkpoint).unwrap();
        let encoded = serde_json::to_string(&entry).unwrap();

        assert!(encoded.contains("recovery-checkpoint"));
        assert!(encoded.contains("tool_execution_start"));
        assert!(encoded.contains("0123456789abcdef"));
        assert!(!encoded.contains(raw_secret_arg));
        assert!(!encoded.contains(raw_tool_output));
    }

    #[test]
    fn append_recovery_checkpoint_persists_redacted_custom_entry() {
        let mut session = SessionManager::in_memory();
        let checkpoint = RecoveryCheckpoint {
            version: 1,
            turn: 1,
            kind: RecoveryCheckpointKind::ToolExecutionEnd,
            tool_call_id: Some("call_456".to_string()),
            tool_name: Some("edit".to_string()),
            args_hash: Some("abcdef0123456789".to_string()),
            success: Some(true),
            error_class: None,
            timestamp: 99,
        };

        let entry_id = session.append_recovery_checkpoint(checkpoint).unwrap();

        assert!(!entry_id.is_empty());
        let entry = session.entries().last().expect("recovery checkpoint entry");
        let SessionEntry::Custom {
            custom_type, data, ..
        } = entry
        else {
            panic!("expected custom recovery checkpoint entry");
        };
        assert_eq!(custom_type, RECOVERY_CHECKPOINT_CUSTOM_TYPE);
        assert_eq!(data["kind"], "tool_execution_end");
        assert_eq!(data["tool_name"], "edit");
        assert_eq!(data["args_hash"], "abcdef0123456789");
        let encoded = serde_json::to_string(data).unwrap();
        assert!(!encoded.contains("oldText"));
        assert!(!encoded.contains("newText"));
    }

    #[test]
    fn recovery_checkpoints_round_trip_into_ledger() {
        let mut session = SessionManager::in_memory();
        let checkpoint = RecoveryCheckpoint {
            version: 1,
            turn: 7,
            kind: RecoveryCheckpointKind::ToolPlanCreated,
            tool_call_id: Some("call_789".to_string()),
            tool_name: Some("read".to_string()),
            args_hash: Some("hash789".to_string()),
            success: Some(true),
            error_class: None,
            timestamp: 100,
        };

        session.append_recovery_checkpoint(checkpoint).unwrap();

        let checkpoints = session.recovery_checkpoints();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].tool_call_id.as_deref(), Some("call_789"));

        let reconciliation = session.recovery_ledger().reconcile_turn(7);
        assert_eq!(reconciliation.retryable_incomplete_tools.len(), 1);
        assert!(reconciliation.unsafe_incomplete_tools.is_empty());
    }
}
