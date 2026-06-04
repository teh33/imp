use std::path::Path;

use async_trait::async_trait;
use imp_llm::truncate_chars_with_suffix;
use serde_json::json;

use super::fuzzy;
use super::{
    generate_diff, line_change_counts, suggest_similar_files, Tool, ToolContext, ToolOutput,
};
use crate::error::Result;
use crate::tools::code_intel;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn label(&self) -> &str {
        "Edit File"
    }
    fn description(&self) -> &str {
        "Edit files by exact replacement, anchored range, or edits[] transaction."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path or default edits[] path" },
                "old_text": { "type": "string", "description": "Text to replace" },
                "new_text": { "type": "string", "description": "Replacement text" },
                "dry_run": {
                    "type": "boolean",
                    "description": "Dry run; return diff only"
                },
                "expected_occurrences": {
                    "type": "integer",
                    "description": "Required exact old_text match count"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all exact matches"
                },
                "anchor_start": {
                    "type": "string",
                    "description": "Start anchor from read(anchors=true)"
                },
                "anchor_end": {
                    "type": "string",
                    "description": "Optional end anchor"
                },
                "target": {
                    "type": "string",
                    "description": "Optional target symbol guard. old_text must occur inside this symbol/block for parseable source files."
                },
                "validate_syntax": {
                    "type": "boolean",
                    "description": "When true, parse the edited source and report syntax errors before apply/during dry run."
                },
                "edits": {
                    "type": "array",
                    "description": "Transactional edits[]",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Per-edit path" },
                            "old_text": { "type": "string" },
                            "new_text": { "type": "string" }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": []
        })
    }
    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        // Multi-edit mode: if `edits` array is present, delegate to MultiEditTool
        if params.get("edits").is_some_and(|v| v.is_array()) {
            return super::multi_edit::MultiEditTool
                .execute(call_id, params, ctx)
                .await;
        }

        let raw_path = params["path"].as_str().unwrap_or("");
        let old_text = get_str_param(&params, "old_text", "oldText").unwrap_or("");
        let new_text = get_str_param(&params, "new_text", "newText").unwrap_or("");
        let dry_run = get_bool_param(&params, "dry_run", "dryRun").unwrap_or(false);
        let replace_all = get_bool_param(&params, "replace_all", "replaceAll").unwrap_or(false);
        let target = params["target"]
            .as_str()
            .filter(|target| !target.trim().is_empty());
        let validate_syntax =
            get_bool_param(&params, "validate_syntax", "validateSyntax").unwrap_or(false);
        let expected_occurrences = params
            .get("expected_occurrences")
            .or_else(|| params.get("expectedOccurrences"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        if raw_path.is_empty() {
            return Ok(ToolOutput::error("Missing required parameter: path"));
        }

        let path = super::resolve_path(&ctx.cwd, raw_path);

        if get_str_param(&params, "anchor_start", "anchorStart").is_some() {
            return execute_anchor_edit(&path, raw_path, &params, ctx).await;
        }

        if old_text.is_empty() {
            return Ok(ToolOutput::error("Missing required parameter: old_text"));
        }

        if let Err(error) = ctx.check_write_path(&path) {
            return Ok(ToolOutput::error(error));
        }
        if !path.exists() {
            let suggestions = suggest_similar_files(&ctx.cwd, raw_path);
            let mut msg = format!("File not found: {}", path.display());
            if !suggestions.is_empty() {
                msg.push_str("\n\nDid you mean:");
                for s in &suggestions {
                    msg.push_str(&format!("\n  {s}"));
                }
            }
            return Ok(ToolOutput::error(msg));
        }

        // Check for unread or stale file — warn but don't block.
        let tracker_warning = {
            let tracker = ctx.file_tracker.lock().ok();
            match tracker {
                Some(t) if !t.was_read(&path) => Some(format!(
                    "Warning: editing {} without reading it first. Consider reading to verify current content.",
                    path.display()
                )),
                Some(t) if t.is_stale(&path) => Some(format!(
                    "Warning: {} was modified externally since last read. Re-read to verify current content.",
                    path.display()
                )),
                _ => None,
            }
        };

        let raw_content = tokio::fs::read_to_string(&path).await?;

        // Normalize to LF for internal processing
        let content = raw_content.replace("\r\n", "\n");
        let has_crlf = raw_content.contains("\r\n");
        let old_normalized = old_text.replace("\r\n", "\n");
        let new_normalized = new_text.replace("\r\n", "\n");

        if let Some(target) = target {
            let Some(block) = code_intel::extract_symbol(&content, &path, target.trim()) else {
                return Ok(ToolOutput::error(format!(
                    "Target symbol not found in {raw_path}: {target}. No changes made."
                )));
            };
            if !block.code.contains(&old_normalized) {
                return Ok(ToolOutput::error(format!(
                    "old_text was not found inside target symbol {target} in {raw_path}. No changes made."
                )));
            }
        }

        let exact_occurrences = count_occurrences(&content, &old_normalized);
        if let Some(expected) = expected_occurrences {
            if exact_occurrences != expected {
                return Ok(ToolOutput::error(format!(
                    "Expected {expected} exact occurrence(s) of old_text in {raw_path}, found {exact_occurrences}. No changes made."
                )));
            }
        }

        let (new_content, was_fuzzy, replacements) = if replace_all {
            if exact_occurrences == 0 {
                return match apply_edit(&content, &old_normalized, &new_normalized) {
                    Ok((_, true)) => Ok(ToolOutput::error(
                        "replaceAll requires exact matches and does not use fuzzy matching. Found 0 exact matches, but a fuzzy match exists. No changes made.",
                    )),
                    Ok(_) => unreachable!("apply_edit cannot exact-match when exact_occurrences is 0"),
                    Err(output) => Ok(output),
                };
            }
            (
                content.replace(&old_normalized, &new_normalized),
                false,
                exact_occurrences,
            )
        } else {
            match apply_edit(&content, &old_normalized, &new_normalized) {
                Ok((new_content, was_fuzzy)) => (new_content, was_fuzzy, 1),
                Err(output) => return Ok(output),
            }
        };

        let syntax_validation =
            validate_syntax.then(|| code_intel::validate_syntax(&new_content, &path));
        if let Some(validation) = &syntax_validation {
            if validation.supported && !validation.valid {
                return Ok(ToolOutput::error(format!(
                    "Edit would introduce syntax errors in {raw_path}: {:?}. No changes made.",
                    validation.errors
                )));
            }
        }
        let symbol_diff = code_intel::diff_top_level_symbols(&content, &new_content, &path);

        let diff = generate_diff(raw_path, &content, &new_content);
        let (lines_added, lines_removed) = line_change_counts(&content, &new_content);

        // Restore original line endings if needed
        let final_content = if has_crlf {
            new_content.replace('\n', "\r\n")
        } else {
            new_content
        };

        if !dry_run {
            ctx.checkpoint_state.snapshot_paths(
                std::slice::from_ref(&path),
                Some(format!("edit {}", path.display())),
            )?;
            tokio::fs::write(&path, &final_content).await?;
        }

        let mut msg = diff;
        if dry_run {
            msg.push_str("\n(dry run: no changes written)");
        }
        if was_fuzzy {
            msg.push_str(
                "\n(matched using fuzzy matching: trailing whitespace/unicode normalized)",
            );
        }
        if let Some(warning) = tracker_warning {
            msg.push('\n');
            msg.push_str(&warning);
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text: msg }],
            details: json!({
                "action": "edit",
                "mode": "single",
                "path": path.display().to_string(),
                "fuzzy_match": was_fuzzy,
                "dry_run": dry_run,
                "replace_all": replace_all,
                "exact_occurrences": exact_occurrences,
                "replacements": replacements,
                "target": target,
                "syntax_validation": syntax_validation.as_ref().map(|validation| json!({
                    "supported": validation.supported,
                    "valid": validation.valid,
                    "language": validation.language,
                    "errors": validation.errors.iter().map(|error| json!({
                        "start_line": error.start_line,
                        "end_line": error.end_line,
                        "kind": error.kind,
                    })).collect::<Vec<_>>(),
                })),
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "files": [{
                    "path": path.display().to_string(),
                    "status": "modified",
                    "lines_added": lines_added,
                    "lines_removed": lines_removed,
                }],
                "changed_symbols": {
                    "added": symbol_diff.added.iter().cloned().collect::<Vec<_>>(),
                    "removed": symbol_diff.removed.iter().cloned().collect::<Vec<_>>(),
                },
            }),
            is_error: false,
        })
    }
}

fn get_str_param<'a>(
    params: &'a serde_json::Value,
    primary: &str,
    legacy: &str,
) -> Option<&'a str> {
    params
        .get(primary)
        .and_then(|v| v.as_str())
        .or_else(|| params.get(legacy).and_then(|v| v.as_str()))
}

fn get_bool_param(params: &serde_json::Value, primary: &str, legacy: &str) -> Option<bool> {
    params
        .get(primary)
        .and_then(|v| v.as_bool())
        .or_else(|| params.get(legacy).and_then(|v| v.as_bool()))
}

async fn execute_anchor_edit(
    path: &Path,
    raw_path: &str,
    params: &serde_json::Value,
    ctx: ToolContext,
) -> Result<ToolOutput> {
    let Some(anchor_start_id) = get_str_param(params, "anchor_start", "anchorStart") else {
        return Ok(ToolOutput::error(
            "Missing required parameter: anchor_start",
        ));
    };
    let anchor_end_id = get_str_param(params, "anchor_end", "anchorEnd").unwrap_or(anchor_start_id);
    let Some(replacement) = get_str_param(params, "new_text", "replacement") else {
        return Ok(ToolOutput::error(
            "Missing required parameter: new_text for anchored edit mode",
        ));
    };
    let dry_run = get_bool_param(params, "dry_run", "dryRun").unwrap_or(false);

    if let Err(error) = ctx.check_write_path(path) {
        return Ok(ToolOutput::error(error));
    }
    if !path.exists() {
        let suggestions = suggest_similar_files(&ctx.cwd, raw_path);
        let mut msg = format!("File not found: {}", path.display());
        if !suggestions.is_empty() {
            msg.push_str("\n\nDid you mean:");
            for s in &suggestions {
                msg.push_str(&format!("\n  {s}"));
            }
        }
        return Ok(ToolOutput::error(msg));
    }

    let Some(start_anchor) = ctx.anchor_store.get(path, anchor_start_id) else {
        return Ok(ToolOutput::error(format!(
            "Anchor not found or expired for {raw_path}: {anchor_start_id}. Re-read with anchors=true before editing."
        )));
    };
    let Some(end_anchor) = ctx.anchor_store.get(path, anchor_end_id) else {
        return Ok(ToolOutput::error(format!(
            "Anchor not found or expired for {raw_path}: {anchor_end_id}. Re-read with anchors=true before editing."
        )));
    };
    if start_anchor.line > end_anchor.line {
        return Ok(ToolOutput::error(
            "anchorStart must refer to a line before or equal to anchorEnd",
        ));
    }

    let raw_content = tokio::fs::read_to_string(path).await?;
    let content = raw_content.replace("\r\n", "\n");
    let has_crlf = raw_content.contains("\r\n");
    let lines = content.lines().collect::<Vec<_>>();
    let start_idx = start_anchor.line.saturating_sub(1);
    let end_idx = end_anchor.line.saturating_sub(1);
    if start_idx >= lines.len() || end_idx >= lines.len() {
        return Ok(ToolOutput::error(
            "Anchor line is outside the current file. Re-read with anchors=true before editing.",
        ));
    }
    if super::stable_hash(lines[start_idx]) != start_anchor.content_hash {
        return Ok(ToolOutput::error(format!(
            "Stale anchor at line {} in {raw_path}. Re-read with anchors=true before editing.",
            start_anchor.line
        )));
    }
    if super::stable_hash(lines[end_idx]) != end_anchor.content_hash {
        return Ok(ToolOutput::error(format!(
            "Stale anchor at line {} in {raw_path}. Re-read with anchors=true before editing.",
            end_anchor.line
        )));
    }

    let mut replacement_normalized = replacement.replace("\r\n", "\n");
    let had_trailing_newline = content.ends_with('\n');
    let mut new_lines = Vec::with_capacity(lines.len() + replacement_normalized.lines().count());
    new_lines.extend_from_slice(&lines[..start_idx]);
    if replacement_normalized.ends_with('\n') {
        replacement_normalized.pop();
    }
    if !replacement_normalized.is_empty() {
        new_lines.extend(replacement_normalized.lines());
    }
    new_lines.extend_from_slice(&lines[end_idx + 1..]);
    let mut new_content = new_lines.join("\n");
    if had_trailing_newline {
        new_content.push('\n');
    }

    let diff = generate_diff(raw_path, &content, &new_content);
    let (lines_added, lines_removed) = line_change_counts(&content, &new_content);
    let final_content = if has_crlf {
        new_content.replace('\n', "\r\n")
    } else {
        new_content.clone()
    };

    if !dry_run {
        ctx.checkpoint_state.snapshot_paths(
            std::slice::from_ref(&path.to_path_buf()),
            Some(format!("anchored edit {}", path.display())),
        )?;
        tokio::fs::write(path, &final_content).await?;
        if let Ok(mut tracker) = ctx.file_tracker.lock() {
            tracker.record_read(path);
        }
    }

    let refreshed_lines = new_content.lines().collect::<Vec<_>>();
    let refreshed =
        ctx.anchor_store
            .record_lines(path, super::stable_hash(&new_content), 1, &refreshed_lines);
    let mut msg = diff;
    if dry_run {
        msg.push_str("\n(dry run: no changes written)");
    }
    msg.push_str("\n(anchored edit: anchors validated before replacement)");

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: msg }],
        details: json!({
            "action": "edit",
            "mode": "anchored",
            "path": path.display().to_string(),
            "dry_run": dry_run,
            "anchored": true,
            "start_line": start_anchor.line,
            "end_line": end_anchor.line,
            "lines_added": lines_added,
            "lines_removed": lines_removed,
            "files": [{
                "path": path.display().to_string(),
                "status": "modified",
                "lines_added": lines_added,
                "lines_removed": lines_removed,
            }],
            "refreshed_anchors": refreshed.iter().map(|anchor| json!({
                "line": anchor.line,
                "anchor": anchor.id,
                "content_hash": format!("{:016x}", anchor.content_hash),
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    })
}

fn count_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    content.match_indices(needle).count()
}

/// Apply a single edit, returning the new content and whether fuzzy matching was used.
/// Extracted so multi_edit can reuse it.
pub(crate) fn apply_edit(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> std::result::Result<(String, bool), ToolOutput> {
    // Try exact match first
    if let Some(pos) = content.find(old_text) {
        let mut result = String::with_capacity(content.len());
        result.push_str(&content[..pos]);
        result.push_str(new_text);
        result.push_str(&content[pos + old_text.len()..]);
        return Ok((result, false));
    }

    // Try fuzzy match
    if let Some(m) = fuzzy::fuzzy_find(content, old_text) {
        let mut result = String::with_capacity(content.len());
        result.push_str(&content[..m.start]);
        result.push_str(new_text);
        result.push_str(&content[m.end..]);
        return Ok((result, true));
    }

    // No match — build helpful error
    let preview = truncate_chars_with_suffix(content, 200, "");
    let msg = format!(
        "Could not find the specified text to replace.\n\
         First 200 chars of file:\n{preview}"
    );
    Err(ToolOutput::error(msg))
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
