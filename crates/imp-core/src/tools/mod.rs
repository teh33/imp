pub mod ask;
pub mod bash;
pub mod code_intel;
pub mod edit;
pub mod git;
pub mod lua;
pub mod memory;
pub mod multi_edit;
pub mod query;
pub mod read;
pub mod scan;
pub mod shell;
pub mod subagent;
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

// ── Truncation helpers ──────────────────────────────────────────────

pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub output_lines: usize,
    pub total_lines: usize,
    pub output_bytes: usize,
    pub total_bytes: usize,
    pub temp_file: Option<PathBuf>,
}

/// Truncate a single line to max_bytes, appending "…" if truncated.
pub fn truncate_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes.min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// Write full content to a temp file, returning the path.
fn write_temp_file(content: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("imp-tools");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("truncated-{}.txt", uuid::Uuid::new_v4());
    let path = dir.join(name);
    std::fs::write(&path, content).ok()?;
    Some(path)
}

/// Truncate keeping the head (first N lines/bytes).
/// When truncated, writes full output to a temp file.
pub fn truncate_head(input: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines: Vec<&str> = input.lines().collect();
    let total_lines = lines.len();
    let total_bytes = input.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: input.to_string(),
            truncated: false,
            output_lines: total_lines,
            total_lines,
            output_bytes: total_bytes,
            total_bytes,
            temp_file: None,
        };
    }

    let mut result = String::new();
    let mut byte_count = 0;
    let mut line_count = 0;

    for line in &lines {
        let line_with_newline = format!("{line}\n");
        if line_count >= max_lines || byte_count + line_with_newline.len() > max_bytes {
            break;
        }
        result.push_str(&line_with_newline);
        byte_count += line_with_newline.len();
        line_count += 1;
    }

    let temp_file = write_temp_file(input);

    TruncationResult {
        content: result,
        truncated: true,
        output_lines: line_count,
        total_lines,
        output_bytes: byte_count,
        total_bytes,
        temp_file,
    }
}

/// Truncate keeping the tail (last N lines/bytes).
/// When truncated, writes full output to a temp file.
pub fn truncate_tail(input: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines: Vec<&str> = input.lines().collect();
    let total_lines = lines.len();
    let total_bytes = input.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: input.to_string(),
            truncated: false,
            output_lines: total_lines,
            total_lines,
            output_bytes: total_bytes,
            total_bytes,
            temp_file: None,
        };
    }

    // Walk backwards from the end, collecting lines that fit.
    let start = total_lines.saturating_sub(max_lines);
    let mut actual_start = start;
    let mut remaining_bytes = max_bytes;

    for (i, line) in lines[start..].iter().enumerate() {
        let line_with_newline = format!("{line}\n");
        if line_with_newline.len() > remaining_bytes {
            actual_start = start + i + 1;
            remaining_bytes = max_bytes;
            // Recalculate from new start
            for line2 in &lines[actual_start..] {
                let l = format!("{line2}\n");
                if l.len() > remaining_bytes {
                    break;
                }
                remaining_bytes -= l.len();
            }
            break;
        }
        remaining_bytes -= line_with_newline.len();
    }

    let mut result = String::new();
    for line in &lines[actual_start..] {
        result.push_str(&format!("{line}\n"));
    }

    let output_lines = total_lines - actual_start;
    let output_bytes = result.len();
    let temp_file = write_temp_file(input);

    TruncationResult {
        content: result,
        truncated: true,
        output_lines,
        total_lines,
        output_bytes,
        total_bytes,
        temp_file,
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

// ── File-not-found suggestions ──────────────────────────────────────

/// Compute the Levenshtein edit distance between two strings.
///
/// Uses a standard DP row-reduction approach — O(m*n) time, O(n) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Search for files with names similar to the missing `target` path.
///
/// Extracts the filename component, walks up to 4 directory levels from `cwd`,
/// and returns up to 3 candidates ranked by Levenshtein distance (closest first).
/// Only files with distance ≤ 3 from the target filename are included.
pub fn suggest_similar_files(cwd: &Path, target: &str) -> Vec<String> {
    let target_name = Path::new(target)
        .file_name()
        .and_then(|n: &std::ffi::OsStr| n.to_str())
        .unwrap_or(target);

    let mut candidates: Vec<(usize, String)> = Vec::new();

    // Skip directories that are typically huge and irrelevant for suggestions
    const SKIP_DIRS: &[&str] = &[
        "target",
        "node_modules",
        ".git",
        "vendor",
        "dist",
        "build",
        "__pycache__",
        ".mypy_cache",
        ".tox",
        ".venv",
    ];

    let walker = walkdir::WalkDir::new(cwd)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    return !SKIP_DIRS.contains(&name);
                }
            }
            true
        })
        .filter_map(|e| e.ok());

    for entry in walker {
        if entry.file_type().is_file() {
            if let Some(name) = entry.file_name().to_str() {
                let dist = levenshtein(target_name, name);
                if dist <= 3 {
                    let rel = entry
                        .path()
                        .strip_prefix(cwd)
                        .unwrap_or(entry.path())
                        .display()
                        .to_string();
                    candidates.push((dist, rel));
                }
            }
        }
    }

    candidates.sort_by_key(|(d, _)| *d);
    candidates.truncate(3);
    candidates.into_iter().map(|(_, p)| p).collect()
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
mod tests {
    use super::*;

    // ── levenshtein ───────────────────────────────────────────────────

    #[test]
    fn suggest_similar_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn suggest_similar_levenshtein_one_substitution() {
        assert_eq!(levenshtein("auth", "aath"), 1);
    }

    #[test]
    fn suggest_similar_levenshtein_one_insertion() {
        assert_eq!(levenshtein("helo", "hello"), 1);
    }

    #[test]
    fn suggest_similar_levenshtein_one_deletion() {
        assert_eq!(levenshtein("hello", "helo"), 1);
    }

    #[test]
    fn suggest_similar_levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn suggest_similar_levenshtein_completely_different() {
        // "abc" vs "xyz": 3 substitutions
        assert_eq!(levenshtein("abc", "xyz"), 3);
    }

    #[test]
    fn suggest_similar_levenshtein_transposition() {
        // "atuh" vs "auth": swap two adjacent chars = distance 2
        assert_eq!(levenshtein("atuh", "auth"), 2);
    }

    // ── suggest_similar_files ─────────────────────────────────────────

    #[test]
    fn suggest_similar_finds_close_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("middleware.rs"), "").unwrap();
        std::fs::write(dir.path().join("unrelated.rs"), "").unwrap();

        let suggestions = suggest_similar_files(dir.path(), "middlewar.rs");
        assert!(
            suggestions.iter().any(|s| s.contains("middleware.rs")),
            "expected middleware.rs in suggestions, got: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_returns_at_most_three() {
        let dir = tempfile::tempdir().unwrap();
        // Create five files each 1 edit away from "xod.rs"
        for name in &["mod.rs", "rod.rs", "cod.rs", "nod.rs", "pod.rs"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }

        let suggestions = suggest_similar_files(dir.path(), "xod.rs");
        assert!(suggestions.len() <= 3);
    }

    #[test]
    fn suggest_similar_nothing_close_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("completely_different.rs"), "").unwrap();

        // "a.rs" is far from "completely_different.rs"
        let suggestions = suggest_similar_files(dir.path(), "a.rs");
        assert!(
            suggestions.is_empty(),
            "expected no suggestions, got: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_similar_ranks_closer_matches_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.rs"), "").unwrap();
        std::fs::write(dir.path().join("autho.rs"), "").unwrap();

        let suggestions = suggest_similar_files(dir.path(), "atuh.rs");
        assert!(!suggestions.is_empty());
        assert!(
            suggestions.iter().any(|s| s.contains("auth.rs")),
            "expected auth.rs, got: {suggestions:?}"
        );
    }

    fn simple_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "count": { "type": "integer" }
            },
            "required": ["path"]
        })
    }

    #[test]
    fn validate_tool_args_valid_passes() {
        let schema = simple_schema();
        let args = serde_json::json!({ "path": "/tmp/foo.txt" });
        assert!(validate_tool_args(&schema, &args).is_ok());
    }

    #[test]
    fn validate_tool_args_valid_with_optional_passes() {
        let schema = simple_schema();
        let args = serde_json::json!({ "path": "/tmp/foo.txt", "count": 5 });
        assert!(validate_tool_args(&schema, &args).is_ok());
    }

    #[test]
    fn validate_tool_args_missing_required_returns_error() {
        let schema = simple_schema();
        // Missing the required "path" field
        let args = serde_json::json!({ "count": 5 });
        let result = validate_tool_args(&schema, &args);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("path") || msg.contains("required"),
            "expected error mentioning 'path' or 'required', got: {msg}"
        );
    }

    #[test]
    fn validate_tool_args_wrong_type_returns_error() {
        let schema = simple_schema();
        // "count" must be integer, not string
        let args = serde_json::json!({ "path": "/tmp/foo.txt", "count": "not-a-number" });
        let result = validate_tool_args(&schema, &args);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("integer") || msg.contains("type"),
            "expected type error, got: {msg}"
        );
    }

    #[test]
    fn validate_tool_args_extra_fields_allowed() {
        // LLMs often add extra fields — we should not reject them
        let schema = simple_schema();
        let args = serde_json::json!({
            "path": "/tmp/foo.txt",
            "llm_added_extra": "some value",
            "another_unknown": 42
        });
        assert!(
            validate_tool_args(&schema, &args).is_ok(),
            "extra/unknown fields should be allowed"
        );
    }

    // ── FileTracker ───────────────────────────────────────────────────

    #[test]
    fn file_track_was_read_false_for_unread_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let tracker = FileTracker::new();
        assert!(!tracker.was_read(&file), "unread file should return false");
    }

    #[test]
    fn file_track_was_read_true_after_recording() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let mut tracker = FileTracker::new();
        tracker.record_read(&file);
        assert!(
            tracker.was_read(&file),
            "file should be marked as read after recording"
        );
    }

    #[test]
    fn file_track_is_stale_false_for_unread_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let tracker = FileTracker::new();
        // Unread file is never stale (no baseline to compare against)
        assert!(!tracker.is_stale(&file));
    }

    #[test]
    fn file_track_is_stale_false_immediately_after_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let mut tracker = FileTracker::new();
        tracker.record_read(&file);
        // No modification since read — should not be stale
        assert!(!tracker.is_stale(&file));
    }

    #[test]
    fn file_track_is_stale_detects_external_modification() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original content").unwrap();

        let mut tracker = FileTracker::new();
        tracker.record_read(&file);

        // Set the file's mtime to 2 seconds in the future to guarantee a detectable change.
        // std::fs::File::set_modified is stable since Rust 1.75 and needs no extra crate.
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&file) {
            let _ = f.set_modified(future);
        }

        assert!(
            tracker.is_stale(&file),
            "file with advanced mtime should be stale"
        );
    }

    // ── FileHistory tests ─────────────────────────────────────

    #[test]
    fn file_history_snapshot_stores_original() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "fn main() {}").unwrap();

        let history = FileHistory::new();
        history.snapshot_before_edit(&file).unwrap();

        assert_eq!(history.original(&file).unwrap(), "fn main() {}");
    }

    #[test]
    fn file_history_second_snapshot_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "original").unwrap();

        let history = FileHistory::new();
        history.snapshot_before_edit(&file).unwrap();

        // Modify the file and snapshot again — should keep original
        std::fs::write(&file, "modified").unwrap();
        history.snapshot_before_edit(&file).unwrap();

        assert_eq!(history.original(&file).unwrap(), "original");
    }

    #[test]
    fn file_history_rollback_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.rs");
        std::fs::write(&file, "original content").unwrap();

        let history = FileHistory::new();
        history.snapshot_before_edit(&file).unwrap();

        std::fs::write(&file, "agent wrote this").unwrap();
        history.rollback(&file).unwrap();

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original content");
    }

    #[test]
    fn file_history_skips_nonexistent_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("does_not_exist.rs");

        let history = FileHistory::new();
        history.snapshot_before_edit(&file).unwrap();

        assert!(history.original(&file).is_none());
    }

    #[test]
    fn file_history_tracked_files_lists_all() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.rs");
        let f2 = dir.path().join("b.rs");
        std::fs::write(&f1, "a").unwrap();
        std::fs::write(&f2, "b").unwrap();

        let history = FileHistory::new();
        history.snapshot_before_edit(&f1).unwrap();
        history.snapshot_before_edit(&f2).unwrap();

        let tracked = history.tracked_files();
        assert_eq!(tracked.len(), 2);
        assert!(tracked.contains(&f1));
        assert!(tracked.contains(&f2));
    }
}
