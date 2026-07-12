//! Scan tool — extract code structure using tree-sitter AST parsing.
//!
//! Dispatches to language-specific parsers based on file extension.
//! Produces rich output: visibility, signatures, fields, variants, trait impls.

pub mod c;
pub mod cpp;
pub mod csharp;
pub mod elixir;
mod extract;
pub mod files;
mod format;
pub mod generic;
pub mod go;
pub mod java;
pub mod kotlin;
mod languages;
pub mod lua;
pub mod ocaml;
pub mod odin;
pub mod perl;
pub mod python;
pub mod ruby;
pub mod rust;
mod search;
pub mod shell;
pub mod swift;
pub mod types;
pub mod typescript;
pub mod zig;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use rayon::prelude::*;
use serde_json::json;

use super::{truncate_head, truncate_line, Tool, ToolContext, ToolOutput, TruncationResult};
use crate::code_index::CodeIndexStore;
use crate::error::{Error, Result};
use crate::repo_index::{RepoSearchHit, RepoStructureIndex};
#[cfg(test)]
use crate::tools::code_intel::{extract_symbol, format_blocks, CodeBlock};
use extract::execute_extract;
#[cfg(test)]
use extract::parse_extract_target;
pub use files::{collect_source_files, is_supported};
use format::{format_result, source_file, truncate_output};
use languages::language_for_extension;
#[cfg(test)]
use search::{
    discover_tests, related_symbols, repo_index_details, repo_index_line, search_index,
    IndexedSymbol,
};
use search::{execute_related, execute_search, execute_tests, symbol_index_cache_key};
use types::*;

/// Node kinds that represent enclosing blocks we want to extract around a line or symbol.
const SUPPORTED_LANGUAGES: &[&str] = &[
    "shell",
    "python",
    "rust",
    "javascript",
    "typescript",
    "go",
    "elixir",
    "ruby",
    "perl",
    "lua",
    "luajit",
    "zig",
    "odin",
    "swift",
    "kotlin",
    "java",
    "c",
    "csharp",
    "cpp",
    "php",
    "scala",
    "dart",
    "ocaml",
];

pub struct ScanTool;

#[async_trait]
impl Tool for ScanTool {
    fn name(&self) -> &str {
        "scan"
    }

    fn label(&self) -> &str {
        "Scan Code Structure"
    }

    fn description(&self) -> &str {
        "Code structure search/extraction with tree-sitter."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["directory", "files", "extract", "search", "tests", "related"],
                    "description": "Scan operation"
                },
                "directory": {
                    "type": "string",
                    "description": "Directory; defaults to cwd"
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files for action=files"
                },
                "targets": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Targets: file#symbol, file:start-end, or file:line"
                },
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "mode": {
                    "type": "string",
                    "enum": ["symbol", "text"],
                    "description": "Search mode; default symbol"
                },
                "target": {
                    "type": "string",
                    "description": "Single target"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Max results"
                }
            },
            "required": ["action"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let action = match params["action"].as_str() {
            Some(a) => a,
            None => return Ok(ToolOutput::error("missing 'action' parameter")),
        };

        let mut files = match action {
            "extract" => {
                let mut targets = parse_string_array(params["targets"].as_array(), "targets")
                    .map_err(Error::Tool)?;
                if let Some(target) = params["target"]
                    .as_str()
                    .map(str::trim)
                    .filter(|target| !target.is_empty())
                {
                    targets.insert(0, target.to_string());
                }
                if targets.is_empty() {
                    return Ok(ToolOutput::error("scan extract requires target or targets"));
                }
                return Ok(execute_extract(&targets, &ctx));
            }
            "search" => {
                let query = match params["query"]
                    .as_str()
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                {
                    Some(query) => query,
                    None => return Ok(ToolOutput::error("scan search requires query")),
                };
                let mode = params["mode"].as_str().unwrap_or("symbol");
                let max_results = params["max_results"].as_u64().unwrap_or(10) as usize;
                let files = files_from_params_or_directory(&params, &ctx)?;
                return Ok(execute_search(
                    files,
                    &ctx.cwd,
                    query,
                    mode,
                    max_results.max(1),
                ));
            }
            "tests" => {
                let targets =
                    parse_string_array(params["targets"].as_array(), "targets").unwrap_or_default();
                let target = params["target"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| targets.first().cloned())
                    .or_else(|| params["query"].as_str().map(str::to_string));
                let Some(target) = target else {
                    return Ok(ToolOutput::error(
                        "scan tests requires target, targets, or query",
                    ));
                };
                let max_results = params["max_results"].as_u64().unwrap_or(10) as usize;
                let files = files_from_params_or_directory(&params, &ctx)?;
                return Ok(execute_tests(files, &ctx.cwd, &target, max_results.max(1)));
            }
            "related" => {
                let targets =
                    parse_string_array(params["targets"].as_array(), "targets").unwrap_or_default();
                let target = params["target"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| targets.first().cloned());
                let Some(target) = target else {
                    return Ok(ToolOutput::error("scan related requires target or targets"));
                };
                let files = files_from_params_or_directory(&params, &ctx)?;
                return Ok(execute_related(files, &ctx.cwd, &target));
            }
            "references" | "impact" => {
                return Ok(ToolOutput::text(
                    "`references` and `impact` are not available in scan. Use `scan related` for grounded local context, `scan tests` for likely tests, or `bash`/`rg` for textual references.",
                ));
            }
            "files" | "build" => {
                let files = match parse_string_array(params["files"].as_array(), "files") {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) => return Ok(ToolOutput::error("scan files requires files")),
                    Err(message) => return Ok(ToolOutput::error(message)),
                };
                files
                    .into_iter()
                    .map(|file| crate::tools::resolve_path(&ctx.cwd, &file))
                    .collect()
            }
            "directory" | "scan" => {
                let dir = params["directory"]
                    .as_str()
                    .map(|d| crate::tools::resolve_path(&ctx.cwd, d))
                    .unwrap_or_else(|| ctx.cwd.clone());
                collect_source_files(&dir)?
            }
            _ => return Ok(ToolOutput::error(format!("unknown action: {action}"))),
        };

        files.sort();
        files.dedup();

        if files.is_empty() {
            return Ok(ToolOutput::text("No supported source files found."));
        }

        let action_name = canonical_action(action);
        if action_name == "directory" {
            if let Some((output, types_count, functions_count)) =
                fast_rust_directory_output(&files, &ctx.cwd)
            {
                return Ok(ToolOutput {
                    content: vec![imp_llm::ContentBlock::Text {
                        text: truncate_output(output),
                    }],
                    details: json!({
                        "action": action_name,
                        "files_analyzed": files.len(),
                        "supported_languages": SUPPORTED_LANGUAGES,
                        "types_count": types_count,
                        "functions_count": functions_count,
                        "fast_path": "rust_directory_skeleton",
                    }),
                    is_error: false,
                });
            }
        }

        let result = extract_files(&files, &ctx.cwd);
        let output = format_result(&result, &files, &ctx.cwd, action_name, None);

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text {
                text: truncate_output(output),
            }],
            details: json!({
                "action": action_name,
                "files_analyzed": files.len(),
                "supported_languages": SUPPORTED_LANGUAGES,
                "types_count": result.types.len(),
                "functions_count": result.functions.len(),
            }),
            is_error: false,
        })
    }
}

// ── parameter helpers ───────────────────────────────────────────────

fn canonical_action(action: &str) -> &str {
    match action {
        "scan" => "directory",
        "build" => "files",
        other => other,
    }
}

fn parse_string_array(
    values: Option<&Vec<serde_json::Value>>,
    field: &str,
) -> std::result::Result<Vec<String>, String> {
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    let mut strings = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let Some(text) = value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Err(format!("{field}[{index}] must be a non-empty string"));
        };
        strings.push(text.to_string());
    }
    Ok(strings)
}

fn files_from_params_or_directory(
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<Vec<PathBuf>> {
    let explicit_files =
        parse_string_array(params["files"].as_array(), "files").map_err(Error::Tool)?;
    if !explicit_files.is_empty() {
        return Ok(explicit_files
            .into_iter()
            .map(|file| crate::tools::resolve_path(&ctx.cwd, &file))
            .collect());
    }

    let dir = params["directory"]
        .as_str()
        .map(|d| crate::tools::resolve_path(&ctx.cwd, d))
        .unwrap_or_else(|| ctx.cwd.clone());
    collect_source_files(&dir)
}

// ── extraction dispatch ─────────────────────────────────────────────

pub fn extract_files(files: &[PathBuf], cwd: &Path) -> ScanResult {
    static CACHE: OnceLock<Mutex<HashMap<u64, ScanResult>>> = OnceLock::new();
    let key = file_set_cache_key(files);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(result) = cache.get(&key) {
            return result.clone();
        }
    }

    if files.len() <= 8 {
        let mut result = ScanResult::default();
        for file in files {
            merge_scan_result_preserving_existing(&mut result, extract_file(file, cwd));
        }
        if let Ok(mut cache) = cache.lock() {
            cache.insert(key, result.clone());
        }
        return result;
    }

    let per_file = files
        .par_iter()
        .map(|file| extract_file(file, cwd))
        .collect::<Vec<_>>();
    let result = per_file
        .into_iter()
        .fold(ScanResult::default(), |mut acc, result| {
            merge_scan_result_preserving_existing(&mut acc, result);
            acc
        });

    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, result.clone());
    }
    result
}

fn extract_file(file: &Path, cwd: &Path) -> ScanResult {
    let mut result = ScanResult::default();
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(_) => return result,
    };

    if source.as_bytes().contains(&0) {
        return result;
    }

    let rel = file
        .strip_prefix(cwd)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();

    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    match ext {
        "rs" => rust::parse(&source, &rel, &mut result),
        "ts" => {
            if !rel.ends_with(".d.ts") {
                typescript::parse(&source, &rel, false, &mut result);
            }
        }
        "tsx" => typescript::parse(&source, &rel, true, &mut result),
        "py" => python::parse(&source, &rel, &mut result),
        "go" => go::parse(&source, &rel, &mut result),
        "kt" | "kts" => kotlin::parse(&source, &rel, &mut result),
        "java" => java::parse(&source, &rel, &mut result),
        "cs" => csharp::parse(&source, &rel, &mut result),
        "c" | "h" => c::parse(&source, &rel, &mut result),
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => {
            cpp::parse(&source, &rel, &mut result)
        }
        "rb" => ruby::parse(&source, &rel, &mut result),
        "ex" | "exs" => elixir::parse(&source, &rel, &mut result),
        "lua" | "luau" => lua::parse(&source, &rel, &mut result),
        "ml" | "mli" => ocaml::parse(&source, &rel, &mut result),
        "zig" | "zon" => zig::parse(&source, &rel, &mut result),
        "odin" => odin::parse(&source, &rel, &mut result),
        "sh" | "bash" | "zsh" | "fish" => shell::parse(&source, &rel, &mut result),
        "pl" | "pm" | "t" => perl::parse(&source, &rel, &mut result),
        "swift" => swift::parse(&source, &rel, &mut result),
        "js" | "jsx" => typescript::parse(&source, &rel, ext == "jsx", &mut result),
        "dart" => generic::parse(
            &source,
            &rel,
            tree_sitter_dart::LANGUAGE.into(),
            &mut result,
        ),
        "php" => generic::parse(
            &source,
            &rel,
            tree_sitter_php::LANGUAGE_PHP.into(),
            &mut result,
        ),
        "scala" | "sc" => generic::parse(
            &source,
            &rel,
            tree_sitter_scala::LANGUAGE.into(),
            &mut result,
        ),
        _ => {
            if let Some(language) = language_for_extension(ext) {
                generic::parse(&source, &rel, language, &mut result);
            }
        }
    }
    result
}

fn merge_scan_result_preserving_existing(acc: &mut ScanResult, result: ScanResult) {
    for (name, info) in result.types {
        acc.types.entry(name).or_insert(info);
    }
    for (name, info) in result.functions {
        acc.functions.entry(name).or_insert(info);
    }
    acc.edges.extend(result.edges);
}

fn file_set_cache_key(files: &[PathBuf]) -> u64 {
    let mut sorted = files.to_vec();
    sorted.sort();
    symbol_index_cache_key(&sorted)
}

fn fast_rust_directory_output(files: &[PathBuf], cwd: &Path) -> Option<(String, usize, usize)> {
    if files.is_empty()
        || !files
            .iter()
            .all(|file| file.extension().and_then(|ext| ext.to_str()) == Some("rs"))
    {
        return None;
    }

    let mut sections = Vec::new();
    let mut types_count = 0;
    let mut functions_count = 0;
    let file_sections = files
        .par_iter()
        .map(|file| {
            let source = std::fs::read_to_string(file).ok()?;
            let rel = file
                .strip_prefix(cwd)
                .unwrap_or(file)
                .to_string_lossy()
                .to_string();
            let mut lines = vec![rel];
            let mut types = 0;
            let mut functions = 0;
            for (idx, line) in source.lines().enumerate() {
                let trimmed = line.trim_start();
                let Some(symbol) = rust_skeleton_line(trimmed) else {
                    continue;
                };
                if symbol.contains("fn ") {
                    functions += 1;
                } else {
                    types += 1;
                }
                lines.push(format!("  {}| {symbol}", idx + 1));
            }
            (lines.len() > 1).then(|| (lines.join("\n"), types, functions))
        })
        .collect::<Vec<_>>();

    for file_section in file_sections.into_iter().flatten() {
        sections.push(file_section.0);
        types_count += file_section.1;
        functions_count += file_section.2;
    }

    if sections.is_empty() {
        None
    } else {
        Some((sections.join("\n\n"), types_count, functions_count))
    }
}

fn rust_skeleton_line(trimmed: &str) -> Option<String> {
    let mut text = trimmed;
    for prefix in ["pub(crate) ", "pub(super) ", "pub "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
            break;
        }
    }
    for keyword in ["struct ", "enum ", "trait ", "impl ", "fn ", "async fn "] {
        if text.starts_with(keyword) {
            let cutoff = text
                .find('{')
                .or_else(|| text.find(';'))
                .unwrap_or(text.len());
            return Some(text[..cutoff].trim_end().to_string());
        }
    }
    None
}

#[cfg(test)]
fn get_parser(path: &Path) -> Option<tree_sitter::Parser> {
    crate::tools::code_intel::parser_for_path(path)
}

#[cfg(test)]
mod tests;
