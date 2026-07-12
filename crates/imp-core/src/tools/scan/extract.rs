use std::path::{Path, PathBuf};

use serde_json::json;

use crate::tools::code_intel::CodeBlock;
use crate::tools::{ToolContext, ToolOutput};

use super::format::truncate_output;

pub(super) enum Locator {
    Line(usize),
    Range(usize, usize),
    Symbol(String),
}

pub(super) fn execute_extract(targets: &[String], ctx: &ToolContext) -> ToolOutput {
    let mut blocks = Vec::new();
    let mut errors = Vec::new();

    for target in targets {
        let Some((file, locator)) = parse_extract_target(target) else {
            errors.push(format!(
                "Invalid target `{target}`. Use file#symbol, file:start-end, or file:line."
            ));
            continue;
        };

        let path = crate::tools::resolve_path(&ctx.cwd, &file);
        let Some(content) = read_text_file(&path) else {
            blocks.push(CodeBlock {
                file: PathBuf::from(&file),
                start_line: 0,
                end_line: 0,
                kind: None,
                symbol: None,
                language: language_for_path(Path::new(&file)).map(str::to_string),
                truncated: false,
                code: format!("Error: could not read {file}"),
            });
            continue;
        };

        let rel_path = path.strip_prefix(&ctx.cwd).unwrap_or(&path).to_path_buf();

        match locator {
            Locator::Line(line) => {
                let line_idx = line.saturating_sub(1);
                if let Some(extracted) = extract_blocks_at_lines(&content, &path, &[line_idx]) {
                    for mut block in extracted {
                        block.file = rel_path.clone();
                        blocks.push(block);
                    }
                } else {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = line_idx.saturating_sub(5);
                    let end = (line_idx + 6).min(lines.len());
                    blocks.push(CodeBlock {
                        file: rel_path.clone(),
                        start_line: start + 1,
                        end_line: end,
                        kind: None,
                        symbol: None,
                        language: language_for_path(&path).map(str::to_string),
                        truncated: false,
                        code: lines[start..end].join("\n"),
                    });
                }
            }
            Locator::Range(start, end) => {
                let lines: Vec<&str> = content.lines().collect();
                let s = start.saturating_sub(1).min(lines.len());
                let e = end.min(lines.len());
                blocks.push(CodeBlock {
                    file: rel_path.clone(),
                    start_line: s + 1,
                    end_line: e,
                    kind: None,
                    symbol: None,
                    language: language_for_path(&path).map(str::to_string),
                    truncated: false,
                    code: lines[s..e].join("\n"),
                });
            }
            Locator::Symbol(name) => {
                if let Some(found) = extract_symbol(&content, &path, &name) {
                    blocks.push(CodeBlock {
                        file: rel_path.clone(),
                        ..found
                    });
                } else {
                    blocks.push(CodeBlock {
                        file: rel_path.clone(),
                        start_line: 0,
                        end_line: 0,
                        kind: None,
                        symbol: Some(name.clone()),
                        language: language_for_path(&path).map(str::to_string),
                        truncated: false,
                        code: format!("Symbol '{name}' not found in {file}"),
                    });
                }
            }
        }
    }

    if blocks.is_empty() && errors.is_empty() {
        return ToolOutput::text("No code blocks found.");
    }

    let mut output = String::new();
    if !blocks.is_empty() {
        output.push_str(&format_blocks(&blocks));
    }
    if !errors.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("Errors:\n");
        for error in &errors {
            output.push_str(&format!("- {error}\n"));
        }
    }

    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: truncate_output(output),
        }],
        details: json!({
            "action": "extract",
            "targets_count": targets.len(),
            "blocks_count": blocks.len(),
            "errors": errors,
            "blocks": blocks.iter().map(block_details).collect::<Vec<_>>(),
        }),
        is_error: blocks.is_empty(),
    }
}

pub(super) fn parse_extract_target(target: &str) -> Option<(String, Locator)> {
    if let Some(hash_pos) = target.rfind('#') {
        let file = target[..hash_pos].to_string();
        let symbol = target[hash_pos + 1..].to_string();
        if !file.is_empty() && !symbol.is_empty() {
            return Some((file, Locator::Symbol(symbol)));
        }
    }

    if let Some(colon_pos) = target.rfind(':') {
        let file = target[..colon_pos].to_string();
        let suffix = &target[colon_pos + 1..];
        if !file.is_empty() && !suffix.is_empty() {
            if let Some(dash_pos) = suffix.find('-') {
                let start = suffix[..dash_pos].parse::<usize>().ok()?;
                let end = suffix[dash_pos + 1..].parse::<usize>().ok()?;
                if start == 0 || end == 0 || start > end {
                    return None;
                }
                return Some((file, Locator::Range(start, end)));
            } else if let Ok(line) = suffix.parse::<usize>() {
                if line == 0 {
                    return None;
                }
                return Some((file, Locator::Line(line)));
            }
        }
    }

    None
}

fn read_text_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn block_details(block: &CodeBlock) -> serde_json::Value {
    crate::tools::code_intel::block_details(block)
}

fn extract_blocks_at_lines(
    source: &str,
    path: &Path,
    match_lines: &[usize],
) -> Option<Vec<CodeBlock>> {
    crate::tools::code_intel::extract_blocks_at_lines(source, path, match_lines)
}

fn extract_symbol(source: &str, path: &Path, name: &str) -> Option<CodeBlock> {
    crate::tools::code_intel::extract_symbol(source, path, name)
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    crate::tools::code_intel::language_for_path(path)
}

fn format_blocks(blocks: &[CodeBlock]) -> String {
    crate::tools::code_intel::format_blocks(blocks)
}
