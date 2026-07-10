use std::path::Path;

use super::super::{
    generate_diff, line_change_counts, suggest_similar_files, ToolContext, ToolOutput,
};
use super::matching::apply_edit;
use super::output::ExactOutput;
use super::{get_bool_param, get_str_param};
use crate::error::Result;
use crate::tools::code_intel;

struct ExactOptions {
    old_text: String,
    new_text: String,
    dry_run: bool,
    replace_all: bool,
    target: Option<String>,
    validate_syntax: bool,
    expected_occurrences: Option<usize>,
}

struct PreparedReplacement {
    content: String,
    was_fuzzy: bool,
    replacements: usize,
    exact_occurrences: usize,
    syntax_validation: Option<code_intel::SyntaxValidation>,
}

pub(super) async fn execute(
    path: &Path,
    raw_path: &str,
    params: &serde_json::Value,
    ctx: ToolContext,
) -> Result<ToolOutput> {
    let options = match ExactOptions::parse(params) {
        Ok(options) => options,
        Err(output) => return Ok(output),
    };
    if let Some(output) = validate_path(path, raw_path, &ctx) {
        return Ok(output);
    }
    let warning = tracker_warning(&ctx, path);
    let raw_content = tokio::fs::read_to_string(path).await?;
    let content = raw_content.replace("\r\n", "\n");
    let replacement = match prepare_replacement(path, raw_path, &content, &options) {
        Ok(replacement) => replacement,
        Err(output) => return Ok(output),
    };
    let symbol_diff = code_intel::diff_top_level_symbols(&content, &replacement.content, path);
    let diff = generate_diff(raw_path, &content, &replacement.content);
    let (added, removed) = line_change_counts(&content, &replacement.content);
    write_replacement(path, &raw_content, &replacement.content, &options, &ctx).await?;
    let message = result_message(diff, warning, &replacement, &options);
    Ok(ExactOutput {
        message,
        path,
        dry_run: options.dry_run,
        replace_all: options.replace_all,
        exact_occurrences: replacement.exact_occurrences,
        replacements: replacement.replacements,
        target: options.target.as_deref(),
        syntax_validation: replacement.syntax_validation,
        lines_added: added,
        lines_removed: removed,
        symbol_diff,
        was_fuzzy: replacement.was_fuzzy,
    }
    .into_tool_output())
}

impl ExactOptions {
    fn parse(params: &serde_json::Value) -> std::result::Result<Self, ToolOutput> {
        let old_text = get_str_param(params, "old_text", "oldText").unwrap_or("");
        if old_text.is_empty() {
            return Err(ToolOutput::error("Missing required parameter: old_text"));
        }
        Ok(Self {
            old_text: old_text.replace("\r\n", "\n"),
            new_text: get_str_param(params, "new_text", "newText")
                .unwrap_or("")
                .replace("\r\n", "\n"),
            dry_run: get_bool_param(params, "dry_run", "dryRun").unwrap_or(false),
            replace_all: get_bool_param(params, "replace_all", "replaceAll").unwrap_or(false),
            target: params["target"]
                .as_str()
                .filter(|v| !v.trim().is_empty())
                .map(str::to_string),
            validate_syntax: get_bool_param(params, "validate_syntax", "validateSyntax")
                .unwrap_or(false),
            expected_occurrences: params
                .get("expected_occurrences")
                .or_else(|| params.get("expectedOccurrences"))
                .and_then(|value| value.as_u64())
                .map(|value| value as usize),
        })
    }
}

fn validate_path(path: &Path, raw_path: &str, ctx: &ToolContext) -> Option<ToolOutput> {
    if let Err(error) = ctx.check_write_path(path) {
        return Some(ToolOutput::error(error));
    }
    (!path.exists()).then(|| ToolOutput::error(missing_file_message(&ctx.cwd, raw_path, path)))
}

fn prepare_replacement(
    path: &Path,
    raw_path: &str,
    content: &str,
    options: &ExactOptions,
) -> std::result::Result<PreparedReplacement, ToolOutput> {
    if let Some(error) = target_error(
        options.target.as_deref(),
        content,
        path,
        raw_path,
        &options.old_text,
    ) {
        return Err(ToolOutput::error(error));
    }
    let exact_occurrences = count_occurrences(content, &options.old_text);
    validate_occurrences(options.expected_occurrences, exact_occurrences, raw_path)?;
    let (content, was_fuzzy, replacements) = replace_content(
        content,
        &options.old_text,
        &options.new_text,
        options.replace_all,
        exact_occurrences,
    )?;
    let syntax_validation = options
        .validate_syntax
        .then(|| code_intel::validate_syntax(&content, path));
    if syntax_validation
        .as_ref()
        .is_some_and(|value| value.supported && !value.valid)
    {
        return Err(ToolOutput::error(format!(
            "Edit would introduce syntax errors in {raw_path}: {:?}. No changes made.",
            syntax_validation.as_ref().unwrap().errors
        )));
    }
    Ok(PreparedReplacement {
        content,
        was_fuzzy,
        replacements,
        exact_occurrences,
        syntax_validation,
    })
}

async fn write_replacement(
    path: &Path,
    raw_content: &str,
    replacement: &str,
    options: &ExactOptions,
    ctx: &ToolContext,
) -> Result<()> {
    if options.dry_run {
        return Ok(());
    }
    ctx.checkpoint_state.snapshot_paths(
        std::slice::from_ref(&path.to_path_buf()),
        Some(format!("edit {}", path.display())),
    )?;
    let content = if raw_content.contains("\r\n") {
        replacement.replace('\n', "\r\n")
    } else {
        replacement.to_string()
    };
    tokio::fs::write(path, content).await?;
    Ok(())
}

fn result_message(
    mut diff: String,
    warning: Option<String>,
    replacement: &PreparedReplacement,
    options: &ExactOptions,
) -> String {
    if options.dry_run {
        diff.push_str("\n(dry run: no changes written)");
    }
    if replacement.was_fuzzy {
        diff.push_str("\n(matched using fuzzy matching: trailing whitespace/unicode normalized)");
    }
    if let Some(warning) = warning {
        diff.push('\n');
        diff.push_str(&warning);
    }
    diff
}

fn validate_occurrences(
    expected: Option<usize>,
    actual: usize,
    raw_path: &str,
) -> std::result::Result<(), ToolOutput> {
    if expected.is_some_and(|expected| actual != expected) {
        return Err(ToolOutput::error(format!(
            "Expected {} exact occurrence(s) of old_text in {raw_path}, found {actual}. No changes made.",
            expected.unwrap()
        )));
    }
    Ok(())
}

fn replace_content(
    content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
    exact_occurrences: usize,
) -> std::result::Result<(String, bool, usize), ToolOutput> {
    if !replace_all {
        return apply_edit(content, old_text, new_text).map(|(content, fuzzy)| (content, fuzzy, 1));
    }
    if exact_occurrences == 0 {
        return match apply_edit(content, old_text, new_text) {
            Ok((_, true)) => Err(ToolOutput::error(
                "replaceAll requires exact matches and does not use fuzzy matching. Found 0 exact matches, but a fuzzy match exists. No changes made.",
            )),
            Ok(_) => unreachable!("apply_edit cannot exact-match when exact_occurrences is 0"),
            Err(output) => Err(output),
        };
    }
    Ok((
        content.replace(old_text, new_text),
        false,
        exact_occurrences,
    ))
}

fn tracker_warning(ctx: &ToolContext, path: &Path) -> Option<String> {
    let tracker = ctx.file_tracker.lock().ok()?;
    if !tracker.was_read(path) {
        return Some(format!(
            "Warning: editing {} without reading it first. Consider reading to verify current content.",
            path.display()
        ));
    }
    tracker.is_stale(path).then(|| {
        format!(
            "Warning: {} was modified externally since last read. Re-read to verify current content.",
            path.display()
        )
    })
}

fn target_error(
    target: Option<&str>,
    content: &str,
    path: &Path,
    raw_path: &str,
    old_text: &str,
) -> Option<String> {
    let target = target?;
    let Some(block) = code_intel::extract_symbol(content, path, target.trim()) else {
        return Some(format!(
            "Target symbol not found in {raw_path}: {target}. No changes made."
        ));
    };
    (!block.code.contains(old_text)).then(|| {
        format!(
            "old_text was not found inside target symbol {target} in {raw_path}. No changes made."
        )
    })
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

fn count_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    content.match_indices(needle).count()
}
