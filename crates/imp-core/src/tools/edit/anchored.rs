use std::path::Path;

use serde_json::json;

use super::super::{
    generate_diff, line_change_counts, suggest_similar_files, LineAnchor, ToolContext, ToolOutput,
};
use super::{get_bool_param, get_str_param};
use crate::error::Result;

struct AnchorOptions<'a> {
    start_id: &'a str,
    end_id: &'a str,
    replacement: &'a str,
    dry_run: bool,
}

pub(super) async fn execute(
    path: &Path,
    raw_path: &str,
    params: &serde_json::Value,
    ctx: ToolContext,
) -> Result<ToolOutput> {
    let options = match AnchorOptions::parse(params) {
        Ok(options) => options,
        Err(output) => return Ok(output),
    };
    if let Some(output) = validate_path(path, raw_path, &ctx) {
        return Ok(output);
    }
    let (start, end) = match resolve_anchors(path, raw_path, &options, &ctx) {
        Ok(anchors) => anchors,
        Err(output) => return Ok(output),
    };
    let raw_content = tokio::fs::read_to_string(path).await?;
    let content = raw_content.replace("\r\n", "\n");
    let (start_idx, end_idx) = match validate_lines(&content, raw_path, &start, &end) {
        Ok(indices) => indices,
        Err(output) => return Ok(output),
    };
    let new_content = replace_anchored_lines(&content, start_idx, end_idx, options.replacement);
    let diff = generate_diff(raw_path, &content, &new_content);
    let (added, removed) = line_change_counts(&content, &new_content);
    persist(path, &raw_content, &new_content, options.dry_run, &ctx).await?;
    Ok(anchor_output(
        path,
        diff,
        new_content,
        options.dry_run,
        start,
        end,
        added,
        removed,
        &ctx,
    ))
}

impl<'a> AnchorOptions<'a> {
    fn parse(params: &'a serde_json::Value) -> std::result::Result<Self, ToolOutput> {
        let Some(start_id) = get_str_param(params, "anchor_start", "anchorStart") else {
            return Err(ToolOutput::error(
                "Missing required parameter: anchor_start",
            ));
        };
        let Some(replacement) = get_str_param(params, "new_text", "replacement") else {
            return Err(ToolOutput::error(
                "Missing required parameter: new_text for anchored edit mode",
            ));
        };
        Ok(Self {
            start_id,
            end_id: get_str_param(params, "anchor_end", "anchorEnd").unwrap_or(start_id),
            replacement,
            dry_run: get_bool_param(params, "dry_run", "dryRun").unwrap_or(false),
        })
    }
}

fn validate_path(path: &Path, raw_path: &str, ctx: &ToolContext) -> Option<ToolOutput> {
    if let Err(error) = ctx.check_write_path(path) {
        return Some(ToolOutput::error(error));
    }
    (!path.exists()).then(|| ToolOutput::error(missing_file_message(&ctx.cwd, raw_path, path)))
}

fn resolve_anchors(
    path: &Path,
    raw_path: &str,
    options: &AnchorOptions<'_>,
    ctx: &ToolContext,
) -> std::result::Result<(LineAnchor, LineAnchor), ToolOutput> {
    let Some(start) = ctx.anchor_store.get(path, options.start_id) else {
        return Err(ToolOutput::error(format!(
            "Anchor not found or expired for {raw_path}: {}. Re-read with anchors=true before editing.", options.start_id
        )));
    };
    let Some(end) = ctx.anchor_store.get(path, options.end_id) else {
        return Err(ToolOutput::error(format!(
            "Anchor not found or expired for {raw_path}: {}. Re-read with anchors=true before editing.", options.end_id
        )));
    };
    if start.line > end.line {
        return Err(ToolOutput::error(
            "anchorStart must refer to a line before or equal to anchorEnd",
        ));
    }
    Ok((start, end))
}

fn validate_lines(
    content: &str,
    raw_path: &str,
    start: &LineAnchor,
    end: &LineAnchor,
) -> std::result::Result<(usize, usize), ToolOutput> {
    let lines = content.lines().collect::<Vec<_>>();
    let start_idx = start.line.saturating_sub(1);
    let end_idx = end.line.saturating_sub(1);
    if start_idx >= lines.len() || end_idx >= lines.len() {
        return Err(ToolOutput::error(
            "Anchor line is outside the current file. Re-read with anchors=true before editing.",
        ));
    }
    for (index, anchor) in [(start_idx, start), (end_idx, end)] {
        if super::super::stable_hash(lines[index]) != anchor.content_hash {
            return Err(ToolOutput::error(format!(
                "Stale anchor at line {} in {raw_path}. Re-read with anchors=true before editing.",
                anchor.line
            )));
        }
    }
    Ok((start_idx, end_idx))
}

async fn persist(
    path: &Path,
    raw_content: &str,
    new_content: &str,
    dry_run: bool,
    ctx: &ToolContext,
) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    ctx.checkpoint_state.snapshot_paths(
        std::slice::from_ref(&path.to_path_buf()),
        Some(format!("anchored edit {}", path.display())),
    )?;
    let content = if raw_content.contains("\r\n") {
        new_content.replace('\n', "\r\n")
    } else {
        new_content.to_string()
    };
    tokio::fs::write(path, content).await?;
    if let Ok(mut tracker) = ctx.file_tracker.lock() {
        tracker.record_read(path);
    }
    Ok(())
}

fn anchor_output(
    path: &Path,
    mut message: String,
    new_content: String,
    dry_run: bool,
    start: LineAnchor,
    end: LineAnchor,
    lines_added: usize,
    lines_removed: usize,
    ctx: &ToolContext,
) -> ToolOutput {
    let lines = new_content.lines().collect::<Vec<_>>();
    let refreshed =
        ctx.anchor_store
            .record_lines(path, super::super::stable_hash(&new_content), 1, &lines);
    if dry_run {
        message.push_str("\n(dry run: no changes written)");
    }
    message.push_str("\n(anchored edit: anchors validated before replacement)");
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: message }],
        details: json!({
            "action": "edit", "mode": "anchored", "path": path.display().to_string(),
            "dry_run": dry_run, "anchored": true, "start_line": start.line,
            "end_line": end.line, "lines_added": lines_added, "lines_removed": lines_removed,
            "files": [{"path": path.display().to_string(), "status": "modified",
                "lines_added": lines_added, "lines_removed": lines_removed}],
            "refreshed_anchors": refreshed.iter().map(|anchor| json!({
                "line": anchor.line, "anchor": anchor.id,
                "content_hash": format!("{:016x}", anchor.content_hash),
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    }
}

fn replace_anchored_lines(
    content: &str,
    start_idx: usize,
    end_idx: usize,
    replacement: &str,
) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut replacement = replacement.replace("\r\n", "\n");
    let mut new_lines = Vec::with_capacity(lines.len() + replacement.lines().count());
    new_lines.extend_from_slice(&lines[..start_idx]);
    if replacement.ends_with('\n') {
        replacement.pop();
    }
    if !replacement.is_empty() {
        new_lines.extend(replacement.lines());
    }
    new_lines.extend_from_slice(&lines[end_idx + 1..]);
    let mut result = new_lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn missing_file_message(cwd: &Path, raw_path: &str, path: &Path) -> String {
    let suggestions = suggest_similar_files(cwd, raw_path);
    let mut message = format!("File not found: {}", path.display());
    if !suggestions.is_empty() {
        message.push_str("\n\nDid you mean:");
        for suggestion in suggestions {
            message.push_str(&format!("\n  {suggestion}"));
        }
    }
    message
}
