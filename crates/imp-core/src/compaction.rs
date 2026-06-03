use std::ops::Range;

use imp_llm::{truncate_chars_with_suffix, ContentBlock, Message};

use crate::context::estimate_tokens;
use crate::error::Result;
use crate::session::{sanitize_messages, SessionEntry, SessionManager};

fn truncate_for_display(text: &str, max_chars: usize) -> String {
    truncate_chars_with_suffix(text, max_chars, "...")
}

/// A grouped assistant-action slice of message history.
///
/// Each group starts at an assistant message and expands backward over any
/// immediately preceding user messages so preserved tails keep the user prompt
/// that led into the preserved assistant work when possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantActionGroup {
    pub range: Range<usize>,
}

/// Strategy selection for compaction execution.
///
/// `Local` is the canonical path and remains the default for correctness.
/// `ProviderNative` is an optional optimization seam for future support of
/// remote/provider-managed compaction or context-editing APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    Local,
    ProviderNative,
}

/// Capability descriptor used to decide whether a provider-specific compaction
/// optimization may be attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionCapabilities<'a> {
    pub provider_id: &'a str,
    pub model_id: &'a str,
    pub allow_provider_native: bool,
}

/// Select the preferred compaction strategy for a provider/model pair.
///
/// For now this always falls back to `Local` unless provider-native compaction
/// is explicitly allowed and the provider matches a known future optimization
/// seam. This keeps the local/manual contract canonical while avoiding TUI- or
/// provider-specific branching throughout the rest of the codebase.
pub fn select_compaction_strategy(capabilities: &CompactionCapabilities<'_>) -> CompactionStrategy {
    if capabilities.allow_provider_native
        && matches!(
            capabilities.provider_id,
            "openai" | "openai-codex" | "anthropic"
        )
    {
        return CompactionStrategy::ProviderNative;
    }
    CompactionStrategy::Local
}
/// Output of the deterministic pre-summary compaction-prep pipeline.
#[derive(Debug, Clone)]
pub struct PreparedCompaction {
    /// Older history reduced into a summarizer-safe form.
    pub summary_input: Vec<Message>,
    /// Recent working context preserved verbatim (after invariant sanitization).
    pub preserved_tail: Vec<Message>,
    /// Index in the original message list where the preserved tail begins.
    pub preserved_tail_start: usize,
    /// Assistant-action groups discovered in the original message list.
    pub groups: Vec<AssistantActionGroup>,
    /// Number of tool result messages whose bodies were replaced with compact
    /// placeholders inside `summary_input`.
    pub shrunk_tool_results: usize,
}

impl PreparedCompaction {
    pub fn should_compact(&self) -> bool {
        !self.summary_input.is_empty()
    }
}

/// Partition a message list into assistant-action groups.
///
/// Groups are defined by assistant message boundaries. For each assistant
/// message, we pull the group start backward across any directly preceding user
/// messages so the user's prompt is preserved with the assistant work when that
/// work survives compaction.
pub fn assistant_action_groups(messages: &[Message]) -> Vec<AssistantActionGroup> {
    let assistant_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| matches!(msg, Message::Assistant(_)).then_some(idx))
        .collect();

    let mut groups = Vec::new();
    for (group_idx, &assistant_idx) in assistant_indices.iter().enumerate() {
        let mut start = assistant_idx;
        while start > 0 {
            match &messages[start - 1] {
                Message::User(_) => start -= 1,
                _ => break,
            }
        }
        let end = assistant_indices
            .get(group_idx + 1)
            .copied()
            .unwrap_or(messages.len());
        groups.push(AssistantActionGroup { range: start..end });
    }

    groups
}

/// Replace tool-result bodies with lightweight placeholders while keeping tool
/// name, truncated arguments, and byte counts for debugging continuity.
pub fn shrink_messages_for_summary(messages: &[Message]) -> (Vec<Message>, usize) {
    let mut shrunk = messages.to_vec();
    let mut args_map = std::collections::HashMap::<String, String>::new();

    for msg in &shrunk {
        if let Message::Assistant(assistant) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall { id, arguments, .. } = block {
                    let args_json = serde_json::to_string(arguments).unwrap_or_default();
                    args_map.insert(id.clone(), truncate_for_display(&args_json, 100));
                }
            }
        }
    }

    let mut shrunk_count = 0;
    for msg in &mut shrunk {
        if let Message::ToolResult(result) = msg {
            let byte_count: usize = result
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.len(),
                    _ => 0,
                })
                .sum();
            let args_summary = args_map
                .get(&result.tool_call_id)
                .map(|s| s.as_str())
                .unwrap_or("");
            let placeholder = format!(
                "[Output omitted — ran {}({}), returned {} bytes]",
                result.tool_name, args_summary, byte_count
            );
            result.content = vec![ContentBlock::Text { text: placeholder }];
            shrunk_count += 1;
        }
    }

    (shrunk, shrunk_count)
}

/// Deterministically prepare history for a later summary-generation step.
///
/// The returned `summary_input` is safe to send to a summarizer: older history
/// is grouped by assistant-action ranges, tool-heavy observations are shrunk,
/// and message-level tool-call/result invariants are sanitized. The preserved
/// tail keeps the last `keep_recent_groups` assistant-action groups verbatim.
pub fn prepare_messages_for_compaction(
    messages: &[Message],
    keep_recent_groups: usize,
) -> PreparedCompaction {
    let groups = assistant_action_groups(messages);

    if groups.len() <= keep_recent_groups {
        let mut preserved_tail = messages.to_vec();
        sanitize_messages(&mut preserved_tail);
        return PreparedCompaction {
            summary_input: Vec::new(),
            preserved_tail,
            preserved_tail_start: 0,
            groups,
            shrunk_tool_results: 0,
        };
    }

    let preserved_tail_start = if keep_recent_groups == 0 {
        messages.len()
    } else {
        groups[groups.len() - keep_recent_groups].range.start
    };

    let summary_prefix = &messages[..preserved_tail_start];
    let preserved_tail_slice = &messages[preserved_tail_start..];

    let (mut summary_input, shrunk_tool_results) = shrink_messages_for_summary(summary_prefix);
    let mut preserved_tail = preserved_tail_slice.to_vec();

    sanitize_messages(&mut summary_input);
    sanitize_messages(&mut preserved_tail);

    PreparedCompaction {
        summary_input,
        preserved_tail,
        preserved_tail_start,
        groups,
        shrunk_tool_results,
    }
}

// ── Compaction summary prompt ──────────────────────────────────────────────

/// Prefix prepended to the summary in the compaction entry so that later
/// context assembly can mark it clearly for the model.
pub const COMPACTION_SUMMARY_PREFIX: &str = "[CONTEXT COMPACTION] Earlier turns were compacted. \
Use the summary below plus the preserved recent messages to continue. \
Avoid repeating completed work:\n";

/// Options for building the LLM summarization prompt.
#[derive(Debug, Clone, Default)]
pub struct SummaryPromptOptions {
    /// Fully replaces imp's built-in summarization instructions when set.
    pub prompt: Option<String>,
    /// Target size for the generated summary.
    pub target_summary_tokens: Option<u32>,
}

/// Build the structured summarization prompt fed to the LLM using optional
/// user-configured summarizer instructions.
pub fn build_summary_prompt_with_options(
    messages: &[Message],
    options: &SummaryPromptOptions,
) -> String {
    let mut serialized = String::new();
    for msg in messages {
        match msg {
            Message::User(user) => {
                let text: String = user
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                serialized.push_str(&format!(
                    "[USER]: {}\n\n",
                    truncate_for_display(&text, 3000)
                ));
            }
            Message::Assistant(assistant) => {
                let mut parts = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text } => {
                            parts.push(truncate_for_display(text, 3000));
                        }
                        ContentBlock::ToolCall {
                            name, arguments, ..
                        } => {
                            let args_str = serde_json::to_string(arguments).unwrap_or_default();
                            parts.push(format!(
                                "[tool call: {}({})]",
                                name,
                                truncate_for_display(&args_str, 500)
                            ));
                        }
                        ContentBlock::Thinking { text } => {
                            parts.push(format!("[thinking: {}]", truncate_for_display(text, 500)));
                        }
                        _ => {}
                    }
                }
                serialized.push_str(&format!("[ASSISTANT]: {}\n\n", parts.join("\n")));
            }
            Message::ToolResult(result) => {
                let text: String = result
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                serialized.push_str(&format!(
                    "[TOOL RESULT {}]: {}\n\n",
                    result.tool_name,
                    truncate_for_display(&text, 3000)
                ));
            }
        }
    }

    if let Some(custom_prompt) = options
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let target = options.target_summary_tokens.unwrap_or(40_000);
        return format!(
            "{custom_prompt}\n\nTURNS TO SUMMARIZE:\n{serialized}\n\nTarget summary size: ~{target} tokens. Write only the summary body."
        );
    }

    let target = options
        .target_summary_tokens
        .map(|tokens| format!("Target about {tokens} tokens unless the history is tiny."))
        .unwrap_or_else(|| "Target 800-1600 words unless the history is tiny.".to_string());

    format!(
        "Create a compact, high-signal handoff summary for a later assistant that will \
         continue this conversation after earlier turns are compacted. Treat this as \
         an operational state transfer, not a narrative transcript.\n\n\
         TURNS TO SUMMARIZE:\n{serialized}\n\
         Required output shape:\n\n\
         ## Goal\n[What the user is trying to accomplish; include explicit user preferences]\n\n\
         ## Current State\n[Where the task stands now; include branch/session status if known]\n\n\
         ## Completed Work\n[Concrete work already done; include file paths, commands run, and results]\n\n\
         ## Key Decisions\n[Important technical/product decisions and why]\n\n\
         ## Relevant Files And Artifacts\n[Files read/modified/created, artifact paths for truncated outputs, links, IDs]\n\n\
         ## Open Questions / Risks\n[Only unresolved issues that matter for continuing correctly]\n\n\
         ## Next Step\n[The single most useful next action]\n\n\
         Rules:\n\
         - Preserve facts needed to resume work without rereading the full transcript.\n\
         - Prefer stable nouns: file paths, symbols, commands, errors, IDs, URLs, model names, settings.\n\
         - Preserve explicit user instructions and corrections verbatim when short.\n\
         - Preserve references to truncated-output artifact files; do not summarize them away.\n\
         - Omit chatter, repeated attempts, and obsolete plans unless they explain current state.\n\
         - Be concise but complete. {target}\n\
         - Do not include any preamble or prefix. Write only the summary body."
    )
}

fn text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assistant_visible_parts(blocks: &[ContentBlock]) -> Vec<String> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(truncate_for_display(trimmed, 900));
                }
            }
            ContentBlock::ToolCall {
                name, arguments, ..
            } => {
                let args = serde_json::to_string(arguments).unwrap_or_default();
                parts.push(format!(
                    "called {name}({})",
                    truncate_for_display(&args, 220)
                ));
            }
            // Thinking traces and non-text blocks are intentionally omitted from
            // deterministic compaction. They are expensive and less valuable
            // than user instructions, visible assistant output, and tool intent.
            ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => {}
        }
    }
    parts
}

fn build_fallback_summary(messages: &[Message]) -> String {
    const MAX_GOAL_LINES: usize = 10;
    const MAX_DIGEST_CHARS: usize = 18_000;
    const MAX_TOOL_LINES: usize = 40;

    let mut goal_lines = Vec::new();
    for msg in messages {
        if goal_lines.len() >= MAX_GOAL_LINES {
            break;
        }
        let Message::User(user) = msg else {
            continue;
        };
        let text = text_blocks(&user.content);
        if text.is_empty() {
            continue;
        }
        goal_lines.push(format!("- {}", truncate_for_display(&text, 700)));
    }

    let mut chronological = Vec::new();
    let mut chronological_chars = 0usize;
    let mut omitted_older = 0usize;
    for msg in messages.iter().rev() {
        let line = match msg {
            Message::User(user) => {
                let text = text_blocks(&user.content);
                (!text.is_empty()).then(|| format!("- User: {}", truncate_for_display(&text, 900)))
            }
            Message::Assistant(assistant) => {
                let parts = assistant_visible_parts(&assistant.content);
                (!parts.is_empty()).then(|| format!("- Assistant: {}", parts.join("; ")))
            }
            Message::ToolResult(result) => Some(format!(
                "- Tool result omitted: {} returned output{}.",
                result.tool_name,
                if result.is_error {
                    " with an error"
                } else {
                    ""
                }
            )),
        };

        let Some(line) = line else {
            continue;
        };
        let line_chars = line.len() + 1;
        if chronological_chars + line_chars > MAX_DIGEST_CHARS {
            omitted_older += 1;
            continue;
        }
        chronological_chars += line_chars;
        chronological.push(line);
    }
    chronological.reverse();

    let mut tool_lines = Vec::new();
    for msg in messages.iter().rev() {
        if tool_lines.len() >= MAX_TOOL_LINES {
            break;
        }
        let Message::Assistant(assistant) = msg else {
            continue;
        };
        for block in assistant.content.iter().rev() {
            let ContentBlock::ToolCall {
                name, arguments, ..
            } = block
            else {
                continue;
            };
            let args = serde_json::to_string(arguments).unwrap_or_default();
            tool_lines.push(format!("- {name}({})", truncate_for_display(&args, 260)));
            if tool_lines.len() >= MAX_TOOL_LINES {
                break;
            }
        }
    }
    tool_lines.reverse();

    let mut sections = Vec::new();
    sections.push("## Goal And User Instructions".to_string());
    if goal_lines.is_empty() {
        sections.push("Continue the conversation using the compacted context below.".to_string());
    } else {
        sections.push(goal_lines.join("\n"));
    }

    sections.push("\n## Recent Working Context".to_string());
    if omitted_older > 0 {
        sections.push(format!(
            "Earlier compactable message(s) omitted from this deterministic digest: {omitted_older}."
        ));
    }
    if chronological.is_empty() {
        sections.push("No text-bearing prior messages were available.".to_string());
    } else {
        sections.push(chronological.join("\n"));
    }

    if !tool_lines.is_empty() {
        sections.push("\n## Recent Tool Calls".to_string());
        sections.push(tool_lines.join("\n"));
    }

    sections.push("\n## Compaction Notes".to_string());
    sections.push(
        "This deterministic compaction retained user prompts, visible assistant output, and tool-call metadata. Tool result bodies, images, and thinking traces were intentionally omitted from active context. Use tools to reread files or artifacts when exact output is needed."
            .to_string(),
    );

    sections.push("\n## Next Step".to_string());
    sections.push("Continue from the recent working context above.".to_string());

    sections.join("\n")
}

// ── Compaction executor ───────────────────────────────────────────────────

/// Default token budget for recent context preserved by automatic compaction.
pub const AUTO_COMPACTION_RECENT_TAIL_TOKENS: u32 = 150_000;

/// Result of deterministic in-memory auto-compaction.
#[derive(Debug, Clone)]
pub struct AutoCompactionResult {
    pub messages: Vec<Message>,
    pub tokens_before: u32,
    pub tokens_after: u32,
}

fn sanitize_message_for_auto_compaction(message: &Message) -> Option<Message> {
    match message {
        Message::User(user) => {
            let content = user
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } if !text.trim().is_empty() => {
                        Some(ContentBlock::Text { text: text.clone() })
                    }
                    ContentBlock::Image { media_type, data } => Some(ContentBlock::Text {
                        text: format!(
                            "[Image omitted during auto-compaction: {media_type}, {} bytes]",
                            data.len()
                        ),
                    }),
                    ContentBlock::Thinking { .. } | ContentBlock::ToolCall { .. } => None,
                    ContentBlock::Text { .. } => None,
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| {
                Message::User(imp_llm::UserMessage {
                    content,
                    timestamp: user.timestamp,
                })
            })
        }
        Message::Assistant(assistant) => {
            let content = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } if !text.trim().is_empty() => {
                        Some(ContentBlock::Text {
                            text: truncate_for_display(text, 2_000),
                        })
                    }
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some(ContentBlock::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: truncate_tool_arguments(arguments),
                    }),
                    ContentBlock::Image { media_type, data } => Some(ContentBlock::Text {
                        text: format!(
                            "[Image omitted during auto-compaction: {media_type}, {} bytes]",
                            data.len()
                        ),
                    }),
                    ContentBlock::Thinking { .. } | ContentBlock::Text { .. } => None,
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| {
                Message::Assistant(imp_llm::AssistantMessage {
                    content,
                    usage: assistant.usage.clone(),
                    stop_reason: assistant.stop_reason.clone(),
                    timestamp: assistant.timestamp,
                })
            })
        }
        Message::ToolResult(result) => Some(Message::ToolResult(imp_llm::ToolResultMessage {
            tool_call_id: result.tool_call_id.clone(),
            tool_name: result.tool_name.clone(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "[Tool output omitted during auto-compaction: {}{}]",
                    result.tool_name,
                    if result.is_error {
                        " returned an error"
                    } else {
                        " succeeded"
                    }
                ),
            }],
            is_error: result.is_error,
            details: result.details.clone(),
            timestamp: result.timestamp,
        })),
    }
}

fn truncate_tool_arguments(arguments: &serde_json::Value) -> serde_json::Value {
    match arguments {
        serde_json::Value::String(text) if text.len() > 1_000 => {
            serde_json::Value::String(truncate_for_display(text, 1_000))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(truncate_tool_arguments)
                .collect::<Vec<_>>(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), truncate_tool_arguments(value)))
                .collect(),
        ),
        _ => arguments.clone(),
    }
}

fn auto_tail_start(messages: &[Message], model: &imp_llm::Model, tail_tokens: u32) -> usize {
    let mut groups = Vec::new();
    let mut start = 0usize;
    for (idx, msg) in messages.iter().enumerate() {
        if idx > start && msg.is_user() {
            groups.push(start..idx);
            start = idx;
        }
    }
    if start < messages.len() {
        groups.push(start..messages.len());
    }

    let mut used = 0u32;
    let mut tail_start = messages.len();
    for group in groups.iter().rev() {
        let group_tokens: u32 = messages[group.clone()]
            .iter()
            .map(|message| crate::context::estimate_message_tokens_for_model(message, &model.meta))
            .sum();
        if used > 0 && used.saturating_add(group_tokens) > tail_tokens {
            break;
        }
        used = used.saturating_add(group_tokens);
        tail_start = group.start;
    }
    tail_start
}

/// Deterministically compact active messages before a provider request.
pub fn compact_messages_for_auto_compaction(
    messages: &[Message],
    model: &imp_llm::Model,
    recent_tail_tokens: u32,
) -> Option<AutoCompactionResult> {
    if messages.len() < 4 {
        return None;
    }

    let tokens_before = crate::context::context_usage(messages, model).used;
    let tail_start = auto_tail_start(messages, model, recent_tail_tokens);
    if tail_start == 0 || tail_start >= messages.len() {
        return None;
    }

    let summary_input = messages[..tail_start]
        .iter()
        .filter_map(sanitize_message_for_auto_compaction)
        .collect::<Vec<_>>();
    let preserved_tail = messages[tail_start..]
        .iter()
        .filter_map(sanitize_message_for_auto_compaction)
        .collect::<Vec<_>>();
    if summary_input.is_empty() || preserved_tail.is_empty() {
        return None;
    }

    let summary_body = build_fallback_summary(&summary_input);
    let mut compacted = vec![Message::user(format!(
        "{COMPACTION_SUMMARY_PREFIX}{summary_body}"
    ))];
    compacted.extend(preserved_tail);

    let tokens_after = crate::context::context_usage(&compacted, model).used;
    (tokens_after < tokens_before).then_some(AutoCompactionResult {
        messages: compacted,
        tokens_before,
        tokens_after,
    })
}

/// Default number of recent assistant-action groups to preserve verbatim.
pub const DEFAULT_KEEP_RECENT_GROUPS: usize = 4;

/// Default for fast local `/compact`: retain all older value in a compact
/// deterministic digest and avoid carrying raw tool outputs/thinking forward.
pub const LOCAL_COMPACTION_KEEP_RECENT_GROUPS: usize = 0;

/// Result of a successful compaction.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_id: String,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub compaction_entry_id: String,
}

/// Execute a manual compaction on the current branch of a session.
///
/// This is the main entry point for `/compact`. It:
/// 1. Prepares the history via the safe deterministic pipeline.
/// 2. Generates a structured summary of the older prefix.
/// 3. Persists a `SessionEntry::Compaction` that partitions the branch.
///
/// The `generate_summary` closure receives the serialized summarization
/// prompt and returns the LLM-generated summary text. Returning `Ok(None)`
/// uses the deterministic fallback summary; returning `Err` surfaces the
/// compaction failure to the caller.
///
/// Returns `None` if there is not enough history to compact.
pub fn execute_manual_compaction<F>(
    session: &mut SessionManager,
    keep_recent_groups: usize,
    generate_summary: F,
) -> Result<Option<CompactionResult>>
where
    F: FnOnce(&str) -> Result<Option<String>>,
{
    execute_manual_compaction_with_prompt_options(
        session,
        keep_recent_groups,
        &SummaryPromptOptions::default(),
        generate_summary,
    )
}

/// Execute manual compaction with explicit summarization prompt options.
pub fn execute_manual_compaction_with_prompt_options<F>(
    session: &mut SessionManager,
    keep_recent_groups: usize,
    prompt_options: &SummaryPromptOptions,
    generate_summary: F,
) -> Result<Option<CompactionResult>>
where
    F: FnOnce(&str) -> Result<Option<String>>,
{
    let raw_messages = session.get_active_messages();
    let tokens_before = raw_messages
        .iter()
        .map(|m| {
            let json = serde_json::to_string(m).unwrap_or_default();
            estimate_tokens(&json)
        })
        .sum();

    let prepared = prepare_messages_for_compaction(&raw_messages, keep_recent_groups);
    if !prepared.should_compact() {
        return Ok(None);
    }

    // Build the summarization prompt from the shrunk older prefix.
    let prompt = build_summary_prompt_with_options(&prepared.summary_input, prompt_options);

    // Call the provided summarizer. If it returns Ok(None), use a bounded deterministic fallback.
    let summary_body = generate_summary(&prompt)?
        .unwrap_or_else(|| build_fallback_summary(&prepared.summary_input));

    let summary_text = format!("{COMPACTION_SUMMARY_PREFIX}{summary_body}");

    // Find the first kept message id from the preserved tail.
    // We need to locate the id in the raw session branch.
    let branch = session.get_branch();
    let first_kept_id = if prepared.preserved_tail_start < raw_messages.len() {
        // Walk the branch to find the entry that corresponds to the preserved
        // tail start index in the active messages.
        let mut msg_idx = 0usize;
        let mut found_id = None;
        for entry in &branch {
            if let SessionEntry::Message { id, .. } = entry {
                if msg_idx == prepared.preserved_tail_start {
                    found_id = Some(id.clone());
                    break;
                }
                msg_idx += 1;
            }
        }
        found_id.unwrap_or_default()
    } else {
        String::new()
    };

    let tokens_after: u32 = {
        let summary_tokens = estimate_tokens(&summary_text);
        let tail_tokens: u32 = prepared
            .preserved_tail
            .iter()
            .map(|m| {
                let json = serde_json::to_string(m).unwrap_or_default();
                estimate_tokens(&json)
            })
            .sum();
        summary_tokens + tail_tokens
    };

    let compaction_entry_id = uuid::Uuid::new_v4().to_string();
    session.append(SessionEntry::Compaction {
        id: compaction_entry_id.clone(),
        parent_id: None,
        summary: summary_text.clone(),
        first_kept_id: first_kept_id.clone(),
        tokens_before,
        tokens_after,
    })?;

    Ok(Some(CompactionResult {
        summary: summary_text,
        first_kept_id,
        tokens_before,
        tokens_after,
        compaction_entry_id,
    }))
}

pub fn execute_compaction_with_retry<F>(
    session: &mut SessionManager,
    keep_recent_groups: usize,
    max_retries: u32,
    generate_summary: F,
) -> Result<Option<CompactionResult>>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    execute_compaction_with_retry_and_prompt_options(
        session,
        keep_recent_groups,
        max_retries,
        &SummaryPromptOptions::default(),
        generate_summary,
    )
}

/// Execute manual compaction with overflow retry and explicit prompt options.
pub fn execute_compaction_with_retry_and_prompt_options<F>(
    session: &mut SessionManager,
    mut keep_recent_groups: usize,
    max_retries: u32,
    prompt_options: &SummaryPromptOptions,
    mut generate_summary: F,
) -> Result<Option<CompactionResult>>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    for attempt in 0..=max_retries {
        let result = execute_manual_compaction_with_prompt_options(
            session,
            keep_recent_groups,
            prompt_options,
            &mut generate_summary,
        )?;
        match result {
            Some(result) => return Ok(Some(result)),
            None if attempt < max_retries => {
                keep_recent_groups += 2;
            }
            None => return Ok(None),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionManager;
    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
    use imp_llm::provider::Provider;
    use imp_llm::{
        AssistantMessage, Model, RequestOptions, StopReason, StreamEvent, ToolResultMessage,
    };
    use std::pin::Pin;
    use std::sync::Arc;

    #[test]
    fn compaction_strategy_defaults_to_local() {
        let caps = CompactionCapabilities {
            provider_id: "anthropic",
            model_id: "claude-sonnet",
            allow_provider_native: false,
        };
        assert_eq!(select_compaction_strategy(&caps), CompactionStrategy::Local);
    }

    #[test]
    fn compaction_strategy_exposes_provider_native_seam_for_supported_providers() {
        let openai = CompactionCapabilities {
            provider_id: "openai-codex",
            model_id: "gpt-5-codex",
            allow_provider_native: true,
        };
        assert_eq!(
            select_compaction_strategy(&openai),
            CompactionStrategy::ProviderNative
        );

        let anthropic = CompactionCapabilities {
            provider_id: "anthropic",
            model_id: "claude-sonnet-4-5",
            allow_provider_native: true,
        };
        assert_eq!(
            select_compaction_strategy(&anthropic),
            CompactionStrategy::ProviderNative
        );
    }

    #[test]
    fn compaction_strategy_keeps_unknown_providers_local() {
        let caps = CompactionCapabilities {
            provider_id: "deepseek",
            model_id: "deepseek-chat",
            allow_provider_native: true,
        };
        assert_eq!(select_compaction_strategy(&caps), CompactionStrategy::Local);
    }

    struct NullProvider;

    #[async_trait]
    impl Provider for NullProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: imp_llm::Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            Box::pin(futures::stream::empty())
        }

        async fn resolve_auth(
            &self,
            _auth: &imp_llm::auth::AuthStore,
        ) -> imp_llm::Result<imp_llm::auth::ApiKey> {
            Ok("test".into())
        }

        fn id(&self) -> &str {
            "null"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    fn test_model() -> Model {
        Model {
            meta: ModelMeta {
                id: "test".into(),
                provider: "test".into(),
                name: "Test".into(),
                context_window: 100_000,
                max_output_tokens: 4096,
                pricing: ModelPricing::default(),
                capabilities: Capabilities::default(),
            },
            provider: Arc::new(NullProvider),
        }
    }

    fn make_user(text: &str) -> Message {
        Message::user(text)
    }

    fn make_assistant_tool_call(
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: call_id.into(),
                name: tool_name.into(),
                arguments: args,
            }],
            usage: None,
            stop_reason: StopReason::ToolUse,
            timestamp: 1000,
        })
    }

    fn make_assistant_text(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 1000,
        })
    }

    fn make_tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.into(),
            tool_name: tool_name.into(),
            content: vec![ContentBlock::Text {
                text: output.into(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: 1000,
        })
    }

    #[test]
    fn summary_prompt_options_use_custom_prompt_and_target_tokens() {
        let messages = vec![
            make_user("Please preserve src/main.rs and the failing cargo test."),
            make_assistant_tool_call(
                "c1",
                "bash",
                serde_json::json!({"command": "cargo test -p imp-core"}),
            ),
            make_tool_result("c1", "bash", "error[E0425]: cannot find value"),
        ];
        let prompt = build_summary_prompt_with_options(
            &messages,
            &SummaryPromptOptions {
                prompt: Some("CUSTOM HANDOFF TEMPLATE".into()),
                target_summary_tokens: Some(12_345),
            },
        );

        assert!(prompt.starts_with("CUSTOM HANDOFF TEMPLATE"));
        assert!(prompt.contains("TURNS TO SUMMARIZE"));
        assert!(prompt.contains("src/main.rs"));
        assert!(prompt.contains("cargo test -p imp-core"));
        assert!(prompt.contains("Target summary size: ~12345 tokens"));
    }

    #[test]
    fn summary_prompt_options_fall_back_to_builtin_for_blank_custom_prompt() {
        let messages = vec![make_user("preserve this goal")];
        let prompt = build_summary_prompt_with_options(
            &messages,
            &SummaryPromptOptions {
                prompt: Some("   \n\t".into()),
                target_summary_tokens: Some(777),
            },
        );

        assert!(prompt.contains("Create a compact, high-signal handoff summary"));
        assert!(prompt.contains("Target about 777 tokens"));
        assert!(prompt.contains("preserve this goal"));
    }

    #[test]
    fn auto_compaction_preserves_token_tail_and_drops_huge_tool_output() {
        let model = test_model();
        let huge_output = "x".repeat(80_000);
        let mut messages = Vec::new();
        for idx in 0..8 {
            let call_id = format!("call-{idx}");
            messages.push(make_user(&format!("inspect src/file_{idx}.rs")));
            messages.push(make_assistant_tool_call(
                &call_id,
                "read",
                serde_json::json!({"path": format!("src/file_{idx}.rs")}),
            ));
            messages.push(make_tool_result(&call_id, "read", &huge_output));
        }
        messages.push(make_user("continue with the latest file"));

        let result = compact_messages_for_auto_compaction(&messages, &model, 4_000)
            .expect("tool-heavy context should compact");

        assert!(result.tokens_after < result.tokens_before);
        let compacted_json = serde_json::to_string(&result.messages).unwrap();
        assert!(compacted_json.contains("CONTEXT COMPACTION"));
        assert!(compacted_json.contains("src/file_7.rs"));
        assert!(compacted_json.contains("continue with the latest file"));
        assert!(!compacted_json.contains(&huge_output));
        assert!(compacted_json.contains("Tool result omitted"));
    }

    #[test]
    fn context_compaction_groups_pull_in_prompting_user_messages() {
        let messages = vec![
            make_user("first prompt"),
            make_assistant_text("first answer"),
            make_user("second prompt"),
            make_assistant_tool_call("c1", "read", serde_json::json!({"path": "src/main.rs"})),
            make_tool_result("c1", "read", "fn main() {}"),
            make_assistant_text("done"),
        ];

        let groups = assistant_action_groups(&messages);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].range, 0..3);
        assert_eq!(groups[1].range, 2..5);
        assert_eq!(groups[2].range, 5..6);
    }

    #[test]
    fn context_compaction_prepare_keeps_recent_groups_verbatim() {
        let messages = vec![
            make_user("prompt 1"),
            make_assistant_text("answer 1"),
            make_user("prompt 2"),
            make_assistant_text("answer 2"),
            make_user("prompt 3"),
            make_assistant_text("answer 3"),
        ];

        let prepared = prepare_messages_for_compaction(&messages, 2);
        assert!(prepared.should_compact());
        assert_eq!(prepared.preserved_tail_start, 2);
        assert_eq!(prepared.summary_input.len(), 2);
        assert_eq!(prepared.preserved_tail.len(), 4);
        match &prepared.preserved_tail[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => assert_eq!(text, "prompt 2"),
                other => panic!("unexpected content: {other:?}"),
            },
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn context_compaction_prepare_shrinks_tool_heavy_prefix() {
        let large_output = "x".repeat(4000);
        let messages = vec![
            make_user("prompt 1"),
            make_assistant_tool_call("c1", "grep", serde_json::json!({"pattern": "foo"})),
            make_tool_result("c1", "grep", &large_output),
            make_user("prompt 2"),
            make_assistant_text("answer 2"),
        ];

        let original_bytes: usize = serde_json::to_string(&messages[..3]).unwrap().len();
        let prepared = prepare_messages_for_compaction(&messages, 1);
        let shrunk_bytes: usize = serde_json::to_string(&prepared.summary_input)
            .unwrap()
            .len();

        assert_eq!(prepared.shrunk_tool_results, 1);
        assert!(shrunk_bytes < original_bytes);
        let tool_result_text = match &prepared.summary_input[2] {
            Message::ToolResult(result) => match result.content.as_slice() {
                [ContentBlock::Text { text }] => text.clone(),
                other => panic!("unexpected tool result content: {other:?}"),
            },
            other => panic!("unexpected summary input message: {other:?}"),
        };
        assert!(tool_result_text.starts_with("[Output omitted"));
        assert!(tool_result_text.contains("grep"));
    }

    #[test]
    fn context_compaction_prepare_sanitizes_unpaired_messages() {
        let messages = vec![
            make_user("prompt 1"),
            make_assistant_tool_call("c1", "grep", serde_json::json!({"pattern": "foo"})),
            make_user("prompt 2"),
            make_assistant_text("answer 2"),
        ];

        let prepared = prepare_messages_for_compaction(&messages, 1);
        assert_eq!(prepared.summary_input.len(), 1);
        match &prepared.summary_input[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => assert_eq!(text, "prompt 1"),
                other => panic!("unexpected content: {other:?}"),
            },
            other => panic!("unexpected summary input: {other:?}"),
        }
    }

    #[test]
    fn context_compaction_prepare_noops_when_history_is_short() {
        let messages = vec![make_user("prompt"), make_assistant_text("answer")];
        let prepared = prepare_messages_for_compaction(&messages, 4);
        assert!(!prepared.should_compact());
        assert!(prepared.summary_input.is_empty());
        assert_eq!(prepared.preserved_tail.len(), 2);
    }

    // ── Executor tests ──────────────────────────────────────────────────

    fn make_session_entry(id: &str, msg: Message) -> SessionEntry {
        SessionEntry::Message {
            id: id.into(),
            parent_id: None,
            message: msg,
        }
    }

    #[test]
    fn compact_executor_passes_prompt_options_to_summarizer() {
        let mut mgr = SessionManager::in_memory();
        for idx in 0..6 {
            mgr.append(SessionEntry::Message {
                id: format!("u{idx}"),
                parent_id: None,
                message: make_user(&format!("preserve custom prompt path src/{idx}.rs")),
            })
            .unwrap();
            mgr.append(SessionEntry::Message {
                id: format!("a{idx}"),
                parent_id: None,
                message: make_assistant_text("ack"),
            })
            .unwrap();
        }

        let mut seen_prompt = String::new();
        let result = execute_manual_compaction_with_prompt_options(
            &mut mgr,
            2,
            &SummaryPromptOptions {
                prompt: Some("CUSTOM EXECUTOR PROMPT".into()),
                target_summary_tokens: Some(9_999),
            },
            |prompt| {
                seen_prompt = prompt.to_string();
                Ok(Some("executor summary".into()))
            },
        )
        .unwrap();

        assert!(result.is_some());
        assert!(seen_prompt.starts_with("CUSTOM EXECUTOR PROMPT"));
        assert!(seen_prompt.contains("Target summary size: ~9999 tokens"));
        assert!(seen_prompt.contains("src/0.rs"));
    }

    #[test]
    fn compact_executor_persists_compaction_entry_and_changes_active_history() {
        let mut mgr = SessionManager::in_memory();
        mgr.append(make_session_entry("u1", make_user("first request")))
            .unwrap();
        mgr.append(make_session_entry(
            "a1",
            make_assistant_text("first answer"),
        ))
        .unwrap();
        mgr.append(make_session_entry("u2", make_user("second request")))
            .unwrap();
        mgr.append(make_session_entry(
            "a2",
            make_assistant_text("second answer"),
        ))
        .unwrap();
        mgr.append(make_session_entry("u3", make_user("third request")))
            .unwrap();
        mgr.append(make_session_entry(
            "a3",
            make_assistant_text("third answer"),
        ))
        .unwrap();

        let raw_before = mgr.get_messages().len();
        assert_eq!(raw_before, 6);

        let result = execute_manual_compaction(&mut mgr, 2, |_prompt| {
            Ok(Some("## Goal\nTest compaction".into()))
        })
        .unwrap();

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.summary.contains("CONTEXT COMPACTION"));
        assert!(result.summary.contains("Test compaction"));
        assert!(result.tokens_before > 0);
        assert!(result.tokens_after > 0);
        assert!(result.tokens_after <= result.tokens_before);

        // Raw messages are still preserved.
        let raw_after = mgr.get_messages().len();
        assert_eq!(raw_after, raw_before);

        // Active messages should now be: summary + preserved tail.
        let active = mgr.get_active_messages();
        assert!(active.len() < raw_before);
        // First active message should be the summary.
        match &active[0] {
            Message::User(user) => match user.content.as_slice() {
                [ContentBlock::Text { text }] => {
                    assert!(text.contains("CONTEXT COMPACTION"));
                }
                other => panic!("unexpected content: {other:?}"),
            },
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn compact_executor_returns_none_for_short_history() {
        let mut mgr = SessionManager::in_memory();
        mgr.append(make_session_entry("u1", make_user("only prompt")))
            .unwrap();
        mgr.append(make_session_entry("a1", make_assistant_text("only answer")))
            .unwrap();

        let result =
            execute_manual_compaction(&mut mgr, 4, |_| Ok(Some("summary".into()))).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn compact_executor_uses_fallback_when_summarizer_returns_none() {
        let mut mgr = SessionManager::in_memory();
        for i in 0..6 {
            let uid = format!("u{i}");
            let aid = format!("a{i}");
            mgr.append(make_session_entry(&uid, make_user(&format!("prompt {i}"))))
                .unwrap();
            mgr.append(make_session_entry(
                &aid,
                make_assistant_text(&format!("answer {i}")),
            ))
            .unwrap();
        }

        let result = execute_manual_compaction(&mut mgr, 2, |_prompt| Ok(None)).unwrap();

        assert!(result.is_some());
        let result = result.unwrap();
        // The bounded fallback preserves high-value context when the LLM summarizer is skipped.
        assert!(result.summary.contains("Goal And User Instructions"));
        assert!(result.summary.contains("Recent Working Context"));
        assert!(result.summary.contains("prompt 0"));
    }

    #[test]
    fn compact_executor_surfaces_summarizer_errors() {
        let mut mgr = SessionManager::in_memory();
        for i in 0..6 {
            let uid = format!("u{i}");
            let aid = format!("a{i}");
            mgr.append(make_session_entry(&uid, make_user(&format!("prompt {i}"))))
                .unwrap();
            mgr.append(make_session_entry(
                &aid,
                make_assistant_text(&format!("answer {i}")),
            ))
            .unwrap();
        }

        let result = execute_manual_compaction(&mut mgr, 2, |_prompt| {
            Err(crate::error::Error::Llm(imp_llm::Error::Provider(
                "summarizer failed".into(),
            )))
        });

        assert!(matches!(
            result,
            Err(crate::error::Error::Llm(imp_llm::Error::Provider(message)))
                if message == "summarizer failed"
        ));
        assert!(mgr.latest_compaction().is_none());
    }
}
