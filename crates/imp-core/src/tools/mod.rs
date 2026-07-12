pub mod ask;
pub mod bash;
pub mod browser;
pub mod code_intel;
pub mod edit;
pub mod git;
pub mod lua;
pub mod multi_edit;
pub mod query;
pub mod read;
pub mod scan;
pub mod shell;
pub mod subagent;
mod suggestions;
pub mod task;
mod truncation;
pub mod web;
pub mod workflow;
pub mod write;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use imp_llm::provider::ToolDefinition;
use imp_llm::{ContentBlock, ToolResultMessage};

use crate::agent::AgentCommand;
use crate::config::AgentMode;
use crate::config::LuaCapabilityPolicy;
use crate::error::Result;
use crate::reference_monitor::ToolMetadata;
use crate::trust::Provenance;
use crate::ui::UserInterface;
use crate::workflow_review::TurnWorkflowReviewAccumulator;

pub use suggestions::{levenshtein, suggest_similar_files};
pub use truncation::{truncate_head, truncate_line, truncate_tail, TruncationResult};

/// Resolve a user-provided path: expands `~` to home dir, resolves relative paths against cwd.
pub fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    if raw == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// A tool that can be invoked by the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in LLM tool calls).
    fn name(&self) -> &str;

    /// Human-readable label.
    fn label(&self) -> &str;

    /// Description shown to the LLM.
    fn description(&self) -> &str;

    /// JSON Schema for parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Whether this tool only reads (no side effects).
    fn is_readonly(&self) -> bool;

    /// Metadata used by the runtime reference monitor.
    fn policy_metadata(&self) -> ToolMetadata {
        ToolMetadata::for_tool_name(self.name(), self.is_readonly())
    }

    /// Whether this specific call only reads state.
    fn is_readonly_call(&self, _params: &serde_json::Value) -> bool {
        self.is_readonly()
    }

    /// Reference-monitor metadata for this specific call.
    fn policy_metadata_for(&self, _params: &serde_json::Value) -> ToolMetadata {
        self.policy_metadata()
    }

    /// Resolve an interactive approval request for this call.
    async fn request_approval(
        &self,
        _params: &serde_json::Value,
        ui: Arc<dyn crate::ui::UserInterface>,
    ) -> ToolApproval {
        if !ui.has_ui() {
            return ToolApproval::denied("interactive approval is unavailable");
        }
        match ui
            .confirm("Approve tool action", "Allow this tool action once?")
            .await
        {
            Some(true) => ToolApproval::approved("once"),
            _ => ToolApproval::denied("user denied or cancelled approval"),
        }
    }

    /// Execute the tool.
    async fn execute(
        &self,
        call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput>;
}

/// Tracks which files have been read in the current session and when.
///
/// Used to warn on edits to unread files and detect external modifications.
pub struct FileTracker {
    reads: HashMap<PathBuf, std::time::SystemTime>,
}

impl Default for FileTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTracker {
    pub fn new() -> Self {
        Self {
            reads: HashMap::new(),
        }
    }

    /// Record that a file was read at the current time.
    pub fn record_read(&mut self, path: &Path) {
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        self.reads.insert(path.to_path_buf(), mtime);
    }

    /// Returns true if the file has been read in this session.
    pub fn was_read(&self, path: &Path) -> bool {
        self.reads.contains_key(path)
    }

    /// Returns true if the file's mtime differs from when it was last read,
    /// indicating an external modification. Returns false if the file was
    /// never read or if the mtime cannot be determined.
    pub fn is_stale(&self, path: &Path) -> bool {
        let Some(&recorded_mtime) = self.reads.get(path) else {
            return false;
        };
        let Ok(current_mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else {
            return false;
        };
        current_mtime != recorded_mtime
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineAnchor {
    pub id: String,
    pub line: usize,
    pub content_hash: u64,
}

#[derive(Debug, Default)]
pub struct AnchorStore {
    files: std::sync::Mutex<HashMap<PathBuf, HashMap<String, LineAnchor>>>,
}

impl AnchorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_lines(
        &self,
        path: &Path,
        file_hash: u64,
        start_line: usize,
        lines: &[&str],
    ) -> Vec<LineAnchor> {
        let anchors = lines
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let line_number = start_line + idx;
                let content_hash = stable_hash(line);
                LineAnchor {
                    id: format!(
                        "a{:016x}{:08x}{:016x}",
                        file_hash, line_number, content_hash
                    ),
                    line: line_number,
                    content_hash,
                }
            })
            .collect::<Vec<_>>();

        if let Ok(mut files) = self.files.lock() {
            let entry = files.entry(path.to_path_buf()).or_default();
            for anchor in &anchors {
                entry.insert(anchor.id.clone(), anchor.clone());
            }
        }

        anchors
    }

    pub fn get(&self, path: &Path, id: &str) -> Option<LineAnchor> {
        self.files
            .lock()
            .ok()?
            .get(path)
            .and_then(|anchors| anchors.get(id).cloned())
    }
}

pub fn stable_hash<T: Hash>(value: T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Cloneable runtime hook for loading Lua extension tools into a registry.
pub type LuaToolLoader = Arc<dyn Fn(&LuaCapabilityPolicy, &mut ToolRegistry) + Send + Sync>;

/// Context provided to tools during execution.
#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub update_tx: tokio::sync::mpsc::Sender<ToolUpdate>,
    pub command_tx: tokio::sync::mpsc::Sender<AgentCommand>,
    pub ui: Arc<dyn UserInterface>,
    pub file_cache: Arc<FileCache>,
    /// Shared checkpoint/file-history state for destructive tool operations.
    pub checkpoint_state: Arc<CheckpointState>,
    /// Tracks file reads for staleness detection and unread-edit warnings.
    pub file_tracker: Arc<std::sync::Mutex<FileTracker>>,
    /// Session-local anchors emitted by read and consumed by anchored edit mode.
    pub anchor_store: Arc<AnchorStore>,
    /// Cloneable Lua extension loader inherited from the parent runtime.
    pub lua_tool_loader: Option<LuaToolLoader>,
    /// Active agent mode — determines which actions are permitted.
    pub mode: AgentMode,
    /// Max lines the read tool may return before truncating. 0 means unlimited.
    pub read_max_lines: usize,
    /// Turn-scoped runtime accumulator for between-turn workflow review packets.
    pub turn_workflow_review: Arc<std::sync::Mutex<TurnWorkflowReviewAccumulator>>,
    /// Resolved runtime config for tool-specific policy checks.
    pub config: Arc<crate::config::Config>,
    /// Per-run tool/write policy layered on top of AgentMode.
    pub run_policy: crate::policy::RunPolicy,
    /// Supporting provenance for content that motivates durable writes in this tool call.
    pub supporting_provenance: Vec<Provenance>,
}

fn lock_unpoisoned<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// In-session file content cache. Avoids re-reading files that haven't changed.
pub struct FileCache {
    entries: std::sync::Mutex<std::collections::HashMap<PathBuf, FileCacheEntry>>,
}

struct FileCacheEntry {
    mtime: std::time::SystemTime,
    content: String,
}

impl Default for FileCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FileCache {
    pub fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Read a file, returning cached content if mtime hasn't changed.
    pub fn read(&self, path: &Path) -> std::io::Result<String> {
        let metadata = std::fs::metadata(path)?;
        let mtime = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);

        {
            let cache = lock_unpoisoned(&self.entries);
            if let Some(entry) = cache.get(path) {
                if entry.mtime == mtime {
                    return Ok(entry.content.clone());
                }
            }
        }

        let content = std::fs::read_to_string(path)?;

        {
            let mut cache = lock_unpoisoned(&self.entries);
            cache.insert(
                path.to_path_buf(),
                FileCacheEntry {
                    mtime,
                    content: content.clone(),
                },
            );
        }

        Ok(content)
    }

    /// Invalidate a cache entry (call after write/edit).
    pub fn invalidate(&self, path: &Path) {
        let mut cache = lock_unpoisoned(&self.entries);
        cache.remove(path);
    }
}

/// Pre-edit file snapshots for rollback safety.
///
/// Before the first edit to any file in a session, stores the original content.
/// If the file didn't exist before the edit, nothing is stored.
/// Enables rollback to pre-session state if the agent makes bad edits.
pub struct FileHistory {
    originals: std::sync::Mutex<HashMap<PathBuf, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRecord {
    pub id: String,
    pub label: Option<String>,
    pub created_at: u64,
    pub files: Vec<PathBuf>,
}

/// Shared session-scoped checkpoint state built on top of FileHistory.
///
/// This keeps the existing rollback primitive as the source of truth for file
/// contents while adding lightweight checkpoint metadata that later layers can
/// persist or surface in the UI.
pub struct CheckpointState {
    history: FileHistory,
    records: std::sync::Mutex<Vec<CheckpointRecord>>,
}

impl Default for CheckpointState {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckpointState {
    pub fn new() -> Self {
        Self {
            history: FileHistory::new(),
            records: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Snapshot a set of existing files and record a checkpoint if any were captured.
    pub fn snapshot_paths(
        &self,
        paths: &[PathBuf],
        label: Option<String>,
    ) -> std::io::Result<Option<CheckpointRecord>> {
        let mut unique = Vec::new();
        for path in paths {
            if !unique.iter().any(|existing: &PathBuf| existing == path) {
                unique.push(path.clone());
            }
        }

        let mut captured = Vec::new();
        for path in unique {
            self.history.snapshot_before_edit(&path)?;
            if self.history.original(&path).is_some() {
                captured.push(path);
            }
        }

        if captured.is_empty() {
            return Ok(None);
        }

        let record = CheckpointRecord {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            created_at: imp_llm::now(),
            files: captured,
        };
        lock_unpoisoned(&self.records).push(record.clone());
        Ok(Some(record))
    }

    pub fn checkpoints(&self) -> Vec<CheckpointRecord> {
        lock_unpoisoned(&self.records).clone()
    }

    pub fn checkpoint(&self, id: &str) -> Option<CheckpointRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.id == id)
            .cloned()
    }

    pub fn restore_checkpoint(&self, id: &str) -> std::io::Result<Vec<PathBuf>> {
        let Some(record) = self.checkpoint(id) else {
            return Ok(Vec::new());
        };

        let mut restored = Vec::new();
        for path in &record.files {
            self.history.rollback(path)?;
            restored.push(path.clone());
        }
        Ok(restored)
    }

    pub fn rollback(&self, path: &Path) -> std::io::Result<()> {
        self.history.rollback(path)
    }

    pub fn tracked_files(&self) -> Vec<PathBuf> {
        self.history.tracked_files()
    }

    pub fn original(&self, path: &Path) -> Option<String> {
        self.history.original(path)
    }
}

impl Default for FileHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl FileHistory {
    pub fn new() -> Self {
        Self {
            originals: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Store original content if not already stored for this path (first edit wins).
    /// Does nothing if the file doesn't exist (new file creation).
    pub fn snapshot_before_edit(&self, path: &Path) -> std::io::Result<()> {
        let canonical = path.to_path_buf();

        let mut originals = lock_unpoisoned(&self.originals);
        if originals.contains_key(&canonical) {
            return Ok(()); // first edit wins
        }
        if canonical.exists() {
            let content = std::fs::read_to_string(&canonical)?;
            originals.insert(canonical, content);
        }
        Ok(())
    }

    /// Get the original content of a file (before any edits in this session).
    pub fn original(&self, path: &Path) -> Option<String> {
        lock_unpoisoned(&self.originals).get(path).cloned()
    }

    /// Rollback a file to its original content.
    pub fn rollback(&self, path: &Path) -> std::io::Result<()> {
        let originals = lock_unpoisoned(&self.originals);
        if let Some(content) = originals.get(path) {
            std::fs::write(path, content)?;
        }
        Ok(())
    }

    /// List all files with snapshots.
    pub fn tracked_files(&self) -> Vec<PathBuf> {
        lock_unpoisoned(&self.originals).keys().cloned().collect()
    }
}

impl ToolContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn check_write_path(&self, path: &Path) -> std::result::Result<(), String> {
        match self.run_policy.check_write_path(&self.cwd, path) {
            crate::policy::WritePolicyDecision::Allowed => Ok(()),
            crate::policy::WritePolicyDecision::Denied(reason) => Err(reason),
        }
    }
}

/// Result of a tool execution.
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    pub details: serde_json::Value,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: serde_json::Value::Null,
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: serde_json::Value::Null,
            is_error: true,
        }
    }

    /// Extract the first text block, if any. Useful for tests.
    pub fn text_content(&self) -> Option<&str> {
        self.content.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn into_tool_result(self, call_id: &str, tool_name: &str) -> ToolResultMessage {
        ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: self.content,
            is_error: self.is_error,
            details: self.details,
            timestamp: imp_llm::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApproval {
    pub approved: bool,
    pub scope: String,
    pub reason: String,
}

impl ToolApproval {
    pub fn approved(scope: impl Into<String>) -> Self {
        Self {
            approved: true,
            scope: scope.into(),
            reason: "user approved tool action".into(),
        }
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            approved: false,
            scope: "none".into(),
            reason: reason.into(),
        }
    }
}

/// Partial update from a running tool (for streaming output).
pub struct ToolUpdate {
    pub content: Vec<ContentBlock>,
    pub details: serde_json::Value,
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    aliases: HashMap<String, String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Register a native Rust tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a compatibility alias for an existing canonical tool name.
    ///
    /// Aliases resolve at execution time but are intentionally omitted from
    /// tool definitions so models see the canonical surface only.
    pub fn register_alias(&mut self, alias: impl Into<String>, canonical: impl Into<String>) {
        self.aliases.insert(alias.into(), canonical.into());
    }

    pub fn extend(&mut self, other: ToolRegistry) {
        for tool in other.tools.into_values() {
            self.register(tool);
        }
        for (alias, canonical) in other.aliases {
            self.register_alias(alias, canonical);
        }
    }

    /// Get a tool by canonical name or compatibility alias.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Some(tool);
        }

        self.aliases
            .get(name)
            .and_then(|canonical| self.tools.get(canonical))
    }

    /// Get a cloned map of all tools, including compatibility aliases.
    pub fn tools_map(&self) -> HashMap<String, Arc<dyn Tool>> {
        let mut map = self.tools.clone();
        for (alias, canonical) in &self.aliases {
            if let Some(tool) = self.tools.get(canonical) {
                map.insert(alias.clone(), Arc::clone(tool));
            }
        }
        map
    }

    /// Get all canonical tool definitions (for LLM context).
    /// Compatibility aliases such as legacy `multi_edit` are intentionally hidden
    /// so models learn one canonical edit surface.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<_> = self
            .tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    pub fn definitions_excluding(&self, excluded: &str) -> Vec<ToolDefinition> {
        self.definitions()
            .into_iter()
            .filter(|definition| definition.name != excluded)
            .collect()
    }

    /// Get only readonly tool definitions (for readonly roles).
    pub fn readonly_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<_> = self
            .tools
            .values()
            .filter(|t| t.is_readonly())
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// List all tool names.
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Retain only tools whose names satisfy the predicate.
    ///
    /// Used by `AgentBuilder` to filter tools based on agent mode before the
    /// agent is handed out to callers.
    pub fn retain<F>(&mut self, predicate: F)
    where
        F: Fn(&str) -> bool,
    {
        self.tools.retain(|name, _| predicate(name));
        self.aliases
            .retain(|_, canonical| self.tools.contains_key(canonical));
    }

    /// Get tool definitions filtered to those allowed by an agent mode.
    ///
    /// For `Full` mode (empty allow-list), returns all definitions.
    /// For all other modes, returns only the intersection.
    pub fn definitions_for_mode(
        &self,
        mode: &crate::config::AgentMode,
    ) -> Vec<imp_llm::provider::ToolDefinition> {
        let mut defs: Vec<_> = self
            .tools
            .values()
            .filter(|t| mode.allows_tool(t.name()))
            .map(|t| imp_llm::provider::ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Lookup reference monitor metadata by canonical name or alias.
    pub fn policy_metadata(&self, name: &str) -> Option<ToolMetadata> {
        self.get(name).map(|tool| tool.policy_metadata())
    }

    /// Lookup reference-monitor metadata for a specific call.
    pub fn policy_metadata_for(
        &self,
        name: &str,
        params: &serde_json::Value,
    ) -> Option<ToolMetadata> {
        self.get(name).map(|tool| tool.policy_metadata_for(params))
    }

    /// Return whether a specific call is safe to parallelize as read-only.
    pub fn is_readonly_call(&self, name: &str, params: &serde_json::Value) -> Option<bool> {
        self.get(name).map(|tool| tool.is_readonly_call(params))
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Fuzzy matching for edit tools ───────────────────────────────────

pub(crate) mod fuzzy {
    /// Normalize text for fuzzy matching: strip trailing whitespace per line,
    /// convert smart quotes and unicode dashes to ASCII equivalents.
    pub fn normalize_for_matching(text: &str) -> String {
        text.lines()
            .map(|line| normalize_unicode(line.trim_end()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn normalize_unicode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' => '\'',
                '\u{201C}' | '\u{201D}' => '"',
                '\u{2013}' | '\u{2014}' => '-',
                '\u{00A0}' | '\u{2003}' | '\u{2002}' | '\u{2009}' => ' ',
                other => other,
            })
            .collect()
    }

    /// Result of a fuzzy find: byte range in original content.
    pub struct FuzzyMatch {
        pub start: usize,
        pub end: usize,
    }

    /// Try to find old_text in content using fuzzy matching.
    /// Works line-by-line: normalizes both sides and does sliding-window
    /// matching over lines, then returns the byte range in the original.
    pub fn fuzzy_find(content: &str, old_text: &str) -> Option<FuzzyMatch> {
        let content_lines: Vec<&str> = content.lines().collect();
        let search_norm = normalize_for_matching(old_text);
        let search_lines: Vec<&str> = search_norm.lines().collect();

        if search_lines.is_empty() {
            return None;
        }

        let content_norm_lines: Vec<String> = content_lines
            .iter()
            .map(|l| normalize_unicode(l.trim_end()))
            .collect();

        if search_lines.len() > content_norm_lines.len() {
            return None;
        }

        // Sliding window over content lines
        let window_size = search_lines.len();
        for start_line in 0..=(content_norm_lines.len() - window_size) {
            let matches = content_norm_lines[start_line..start_line + window_size]
                .iter()
                .zip(search_lines.iter())
                .all(|(content_line, search_line)| content_line == search_line);

            if matches {
                // Calculate byte offsets in original content
                let byte_start: usize = content_lines[..start_line]
                    .iter()
                    .map(|l| l.len() + 1) // +1 for \n
                    .sum();

                let end_line = start_line + window_size - 1;
                let byte_end: usize = content_lines[..end_line]
                    .iter()
                    .map(|l| l.len() + 1)
                    .sum::<usize>()
                    + content_lines[end_line].len();

                return Some(FuzzyMatch {
                    start: byte_start,
                    end: byte_end,
                });
            }
        }

        None
    }
}

// ── Diff generation ─────────────────────────────────────────────────

/// Generate a unified diff between old and new content.
pub fn line_change_counts(old: &str, new: &str) -> (usize, usize) {
    use similar::ChangeTag;

    let diff = similar::TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

pub fn generate_diff(file_path: &str, old: &str, new: &str) -> String {
    use similar::TextDiff;

    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();
    output.push_str(&format!("--- {file_path}\n"));
    output.push_str(&format!("+++ {file_path}\n"));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        output.push_str(&format!("{hunk}"));
    }

    output
}

// ── Tool argument validation ─────────────────────────────────────────

/// Validate tool arguments against a JSON Schema.
///
/// Returns `Ok(())` if args are valid, or `Err` with a human-readable
/// description of what failed. Extra/unknown fields are permitted — LLMs often
/// include them and tools should be lenient on input.
pub fn validate_tool_args(schema: &serde_json::Value, args: &serde_json::Value) -> Result<()> {
    use jsonschema::Validator;

    let validator = Validator::new(schema)
        .map_err(|e| crate::error::Error::Tool(format!("Invalid tool schema: {e}")))?;

    let errors: Vec<String> = validator
        .iter_errors(args)
        .map(|e| format!("{e}"))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(crate::error::Error::Tool(format!(
            "Tool argument validation failed:\n{}",
            errors.join("\n")
        )))
    }
}

#[cfg(test)]
mod tests;
