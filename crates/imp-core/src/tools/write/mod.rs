use async_trait::async_trait;
use serde_json::json;

use super::{line_change_counts, truncate_head, Tool, ToolContext, ToolOutput};
use crate::config::WriteOverwritePolicy;
use crate::error::Result;
use crate::tools::code_intel;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn label(&self) -> &str {
        "Write File"
    }
    fn description(&self) -> &str {
        "Create or overwrite a file. Creates parent dirs automatically."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
                "mode": {
                    "type": "string",
                    "enum": ["create", "overwrite"],
                    "description": "Optional safety mode. create fails if the file exists; overwrite makes replacement explicit. Omitted preserves existing behavior."
                },
                "validate_syntax": {
                    "type": "boolean",
                    "description": "When true, parse source content before writing and reject syntax errors for supported languages."
                }
            },
            "required": ["path", "content"]
        })
    }
    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let raw_path = params["path"].as_str().unwrap_or("");
        let content = params["content"].as_str().unwrap_or("");
        let mode = params["mode"].as_str();
        let validate_syntax = params["validate_syntax"]
            .as_bool()
            .or_else(|| params["validateSyntax"].as_bool())
            .unwrap_or(false);

        if raw_path.is_empty() {
            return Ok(ToolOutput::error("Missing required parameter: path"));
        }

        let path = super::resolve_path(&ctx.cwd, raw_path);

        if let Err(error) = ctx.check_write_path(&path) {
            return Ok(ToolOutput::error(error));
        }

        if path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "Path is a directory, not a file: {}",
                path.display()
            )));
        }

        let existed = path.exists();
        if matches!(mode, Some("create")) && existed {
            return Ok(ToolOutput::error(format!(
                "Write mode create refuses to overwrite existing file: {}",
                path.display()
            )));
        }

        let overwrite_check = if existed {
            evaluate_overwrite_policy(&path, &ctx)
        } else {
            OverwriteCheck::default()
        };
        if let Some(error) = overwrite_check.error {
            return Ok(ToolOutput::error(error));
        }

        let before_content = if existed {
            tokio::fs::read_to_string(&path).await.ok()
        } else {
            None
        };

        // Create parent directories
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let checkpoint = if existed {
            ctx.checkpoint_state.snapshot_paths(
                std::slice::from_ref(&path),
                Some(format!("write {}", path.display())),
            )?
        } else {
            None
        };

        // Detect existing line endings to preserve them, default to LF for new files
        let normalized = if existed {
            if let Ok(existing) = tokio::fs::read(&path).await {
                let has_crlf = existing.windows(2).any(|w| w == b"\r\n");
                if has_crlf {
                    // Preserve CRLF: ensure content uses CRLF
                    let lf_content = content.replace("\r\n", "\n");
                    lf_content.replace('\n', "\r\n")
                } else {
                    // LF or no newlines — ensure LF
                    content.replace("\r\n", "\n")
                }
            } else {
                content.replace("\r\n", "\n")
            }
        } else {
            content.replace("\r\n", "\n")
        };

        let syntax_validation =
            validate_syntax.then(|| code_intel::validate_syntax(&normalized, &path));
        if let Some(validation) = &syntax_validation {
            if validation.supported && !validation.valid {
                return Ok(ToolOutput::error(format!(
                    "Write would create syntax errors in {raw_path}: {:?}. No changes made.",
                    validation.errors
                )));
            }
        }
        let symbol_diff = before_content
            .as_deref()
            .map(|before| code_intel::diff_top_level_symbols(before, &normalized, &path));

        let bytes_written = normalized.len();
        let (lines_added, lines_removed) = if let Some(before) = before_content.as_deref() {
            line_change_counts(before, &normalized)
        } else {
            (normalized.lines().count(), 0)
        };
        tokio::fs::write(&path, &normalized).await?;

        let action = if existed { "overwritten" } else { "created" };
        let display = path.display().to_string();
        let summary = format!("{display}: {bytes_written} bytes {action}");

        const DISPLAY_MAX_LINES: usize = 40;
        const DISPLAY_MAX_BYTES: usize = 8_000;
        let display_source = normalized.replace("\r\n", "\n");
        let display_result = truncate_head(&display_source, DISPLAY_MAX_LINES, DISPLAY_MAX_BYTES);
        let display_content = display_result.content.trim_end_matches('\n').to_string();
        let display_note = if display_result.truncated {
            let note = format!(
                "[output truncated: showing {}/{} lines, {}/{} bytes]",
                display_result.output_lines,
                display_result.total_lines,
                display_result.output_bytes,
                display_result.total_bytes,
            );
            if let Some(ref tf) = display_result.temp_file {
                format!("{note} full output: {}", tf.display())
            } else {
                note
            }
        } else {
            String::new()
        };

        let warnings = overwrite_check.warning_messages;
        let warning_codes = overwrite_check.warning_codes;

        let mut text = summary.clone();
        for warning in &warnings {
            text.push('\n');
            text.push_str(warning);
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text }],
            details: json!({
                "action": action,
                "path": display,
                "bytes_written": bytes_written,
                "line_ending": if normalized.contains("\r\n") { "crlf" } else { "lf" },
                "created": !existed,
                "overwritten": existed,
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "files": [{
                    "path": display,
                    "status": if existed { "modified" } else { "created" },
                    "lines_added": lines_added,
                    "lines_removed": lines_removed,
                }],
                "checkpoint_id": checkpoint.as_ref().map(|c| c.id.clone()),
                "checkpoint_label": checkpoint.as_ref().and_then(|c| c.label.clone()),
                "summary": summary,
                "warnings": warnings,
                "warning_codes": warning_codes,
                "overwrite_policy": ctx.config.write.overwrite_policy,
                "mode": mode,
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
                "symbol_diff": symbol_diff.as_ref().map(|diff| json!({
                    "added": diff.added.iter().cloned().collect::<Vec<_>>(),
                    "removed": diff.removed.iter().cloned().collect::<Vec<_>>(),
                    "before": diff.before.iter().cloned().collect::<Vec<_>>(),
                    "after": diff.after.iter().cloned().collect::<Vec<_>>(),
                })),
                "display_content": display_content,
                "display_note": display_note,
            }),
            is_error: false,
        })
    }
}

#[derive(Default)]
struct OverwriteCheck {
    warning_messages: Vec<String>,
    warning_codes: Vec<&'static str>,
    error: Option<String>,
}

fn evaluate_overwrite_policy(path: &std::path::Path, ctx: &ToolContext) -> OverwriteCheck {
    let Ok(tracker) = ctx.file_tracker.lock() else {
        return OverwriteCheck::default();
    };

    let was_read = tracker.was_read(path);
    let is_stale = tracker.is_stale(path);
    let policy = ctx.config.write.overwrite_policy;

    if matches!(policy, WriteOverwritePolicy::Deny) {
        return OverwriteCheck {
            error: Some(format!(
                "Overwriting existing files is disabled by write overwrite policy: {}",
                path.display()
            )),
            ..OverwriteCheck::default()
        };
    }

    if matches!(policy, WriteOverwritePolicy::RequireRead) && !was_read {
        return OverwriteCheck {
            error: Some(format!(
                "Write overwrite policy requires reading the file before overwriting: {}",
                path.display()
            )),
            ..OverwriteCheck::default()
        };
    }

    if matches!(
        policy,
        WriteOverwritePolicy::RequireRead | WriteOverwritePolicy::BlockStale
    ) && is_stale
    {
        return OverwriteCheck {
            error: Some(format!(
                "Write overwrite policy blocks overwriting stale files. Re-read before overwriting: {}",
                path.display()
            )),
            ..OverwriteCheck::default()
        };
    }

    let mut check = OverwriteCheck::default();
    if !was_read {
        check.warning_codes.push("unread_overwrite");
        check.warning_messages.push(format!(
            "Warning: overwriting {} without reading it first. Consider reading to verify current content.",
            path.display()
        ));
    } else if is_stale {
        check.warning_codes.push("stale_overwrite");
        check.warning_messages.push(format!(
            "Warning: {} was modified externally since last read. Re-read to verify current content.",
            path.display()
        ));
    }

    check
}

#[cfg(test)]
mod tests;
