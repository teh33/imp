//! Scan tool — extract code structure using tree-sitter AST parsing.
//!
//! Dispatches to language-specific parsers based on file extension.
//! Produces rich output: visibility, signatures, fields, variants, trait impls.

pub mod c;
pub mod cpp;
pub mod csharp;
pub mod elixir;
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
use crate::tools::code_intel::CodeBlock;
pub use files::{collect_source_files, is_supported};
use format::{format_result, source_file, truncate_output};
use languages::language_for_extension;
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
                    .map_err(|message| Error::Tool(message))?;
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

// ── search and test discovery ───────────────────────────────────────

#[derive(Debug, Clone)]
struct IndexedSymbol {
    file: String,
    name: String,
    kind: String,
    line: usize,
    text: String,
    is_test: bool,
}

#[derive(Debug, Clone)]
struct SearchHit {
    file: String,
    symbol: Option<String>,
    kind: String,
    line: usize,
    score: i32,
    why: Vec<String>,
}

const FULL_SYMBOL_SEARCH_INDEX_LIMIT: usize = 1_000;

fn search_index_files(files: &[PathBuf], query: &str, mode: &str) -> Vec<PathBuf> {
    if mode == "symbol" && files.len() <= FULL_SYMBOL_SEARCH_INDEX_LIMIT {
        return files.to_vec();
    }

    let selected = prefilter_search_files(files, query, mode);
    if mode == "symbol" && selected.is_empty() {
        files.to_vec()
    } else {
        selected
    }
}

fn execute_search(
    mut files: Vec<PathBuf>,
    cwd: &Path,
    query: &str,
    mode: &str,
    max_results: usize,
) -> ToolOutput {
    files.sort();
    files.dedup();
    let index_files = search_index_files(&files, query, mode);
    let repo_index = load_or_build_repo_structure_index(&index_files, cwd);
    let repo_hits = repo_index.search(query, max_results);
    if !repo_hits.is_empty() {
        return execute_search_repo_index(
            files.len(),
            index_files.len(),
            query,
            mode,
            &repo_index,
            &repo_hits,
        );
    }

    let index = build_symbol_index(&index_files, cwd);
    let hits = search_index(&index, query, mode, max_results);
    let mut lines = vec![
        format!("Action: search"),
        format!("Query: {query}"),
        format!("Mode: {mode}"),
        format!("Files analyzed: {}", files.len()),
        format!("Files indexed: {}", index_files.len()),
        repo_index_line(&index),
    ];
    if hits.is_empty() {
        lines.push("No matching symbols found.".to_string());
    } else {
        lines.push("Results:".to_string());
        for hit in &hits {
            let symbol = hit.symbol.as_deref().unwrap_or("<file>");
            lines.push(format!(
                "- {}:{} [{}] {} score={} — {}",
                hit.file,
                hit.line,
                hit.kind,
                symbol,
                hit.score,
                hit.why.join(", ")
            ));
        }
    }

    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: truncate_output(lines.join("\n")),
        }],
        details: json!({
            "action": "search",
            "query": query,
            "mode": mode,
            "files_analyzed": files.len(),
            "repo_intelligence": repo_index_details(&index),
            "results": hits.iter().map(|hit| json!({
                "file": hit.file,
                "symbol": hit.symbol,
                "kind": hit.kind,
                "line": hit.line,
                "score": hit.score,
                "why": hit.why,
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    }
}

fn execute_search_repo_index(
    files_analyzed: usize,
    files_indexed: usize,
    query: &str,
    mode: &str,
    index: &RepoStructureIndex,
    hits: &[RepoSearchHit],
) -> ToolOutput {
    let mut lines = vec![
        format!("Action: search"),
        format!("Query: {query}"),
        format!("Mode: {mode}"),
        format!("Files analyzed: {files_analyzed}"),
        format!("Files indexed: {files_indexed}"),
        repo_structure_index_line(index),
    ];
    lines.push("Results:".to_string());
    for hit in hits {
        let line = hit
            .node
            .location
            .range
            .as_ref()
            .map(|range| range.start.line)
            .unwrap_or(1);
        lines.push(format!(
            "- {}:{} [{}] {} score={} — {}",
            hit.node.location.path,
            line,
            scan_symbol_kind_label(hit.node.symbol.kind),
            hit.node.symbol.name,
            hit.score,
            hit.why.join(", ")
        ));
    }

    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: truncate_output(lines.join("\n")),
        }],
        details: json!({
            "action": "search",
            "query": query,
            "mode": mode,
            "files_analyzed": files_analyzed,
            "files_indexed": files_indexed,
            "index_source": "repo_structure",
            "repo_intelligence": repo_structure_index_details(index),
            "results": hits.iter().map(|hit| {
                let line = hit.node.location.range.as_ref().map(|range| range.start.line).unwrap_or(1);
                json!({
                    "file": hit.node.location.path,
                    "symbol": hit.node.symbol.name,
                    "qualified_symbol": hit.node.qualified_name,
                    "kind": scan_symbol_kind_label(hit.node.symbol.kind),
                    "line": line,
                    "score": hit.score,
                    "why": hit.why,
                    "extract_target": format!("{}#{}", hit.node.location.path, hit.node.symbol.name),
                })
            }).collect::<Vec<_>>(),
        }),
        is_error: false,
    }
}

fn execute_tests(
    mut files: Vec<PathBuf>,
    cwd: &Path,
    target: &str,
    max_results: usize,
) -> ToolOutput {
    files.sort();
    files.dedup();
    let index_files = prefilter_test_files(&files, target);
    let index = build_symbol_index(&index_files, cwd);
    let tests = discover_tests(&index, target, cwd, max_results);
    let mut lines = vec![
        format!("Action: tests"),
        format!("Target: {target}"),
        format!("Files analyzed: {}", files.len()),
    ];
    if tests.is_empty() {
        lines.push("No likely tests found.".to_string());
    } else {
        lines.push("Likely tests:".to_string());
        for test in &tests {
            lines.push(format!(
                "- {}:{} {} — {}",
                test.file,
                test.line,
                test.name,
                test.command.as_deref().unwrap_or("no command inferred")
            ));
        }
    }

    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: truncate_output(lines.join("\n")),
        }],
        details: json!({
            "action": "tests",
            "target": target,
            "files_analyzed": files.len(),
            "tests": tests.iter().map(|test| json!({
                "file": test.file,
                "symbol": test.name,
                "line": test.line,
                "command": test.command,
                "why": test.why,
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    }
}

fn prefilter_test_files(files: &[PathBuf], target: &str) -> Vec<PathBuf> {
    let (target_file, target_symbol) = split_target(target);
    let terms = target_symbol
        .as_deref()
        .map(query_terms)
        .unwrap_or_default();
    let selected = files
        .par_iter()
        .filter_map(|file| {
            let file_text = file.to_string_lossy().to_lowercase();
            let rel_text = file_text.as_str();
            let matches_target_file = target_file
                .as_ref()
                .is_some_and(|target| rel_text.ends_with(&target.to_lowercase()));
            let likely_test_neighbor = target_file
                .as_ref()
                .is_some_and(|target| same_stem_or_test_neighbor(rel_text, target));
            let likely_test = looks_like_test_file(rel_text);
            let matches_terms =
                !terms.is_empty() && terms.iter().any(|term| rel_text.contains(term));
            if matches_target_file || likely_test_neighbor || matches_terms {
                return Some(file.clone());
            }
            if likely_test
                && (terms.is_empty()
                    || std::fs::read_to_string(file)
                        .map(|source| {
                            let source = source.to_lowercase();
                            terms.iter().any(|term| source.contains(term))
                        })
                        .unwrap_or(false))
            {
                return Some(file.clone());
            }
            None
        })
        .collect::<Vec<_>>();

    if selected.is_empty() {
        files.to_vec()
    } else {
        dedup_paths(selected)
    }
}

fn dedup_paths(mut files: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    files.retain(|file| seen.insert(file.clone()));
    files
}

fn prefilter_related_files(files: &[PathBuf], target: &str) -> Vec<PathBuf> {
    let mut selected = prefilter_test_files(files, target);
    let (target_file, target_symbol) = split_target(target);
    let terms = target_symbol
        .as_deref()
        .map(query_terms)
        .unwrap_or_default();
    let mut seen = selected.iter().cloned().collect::<HashSet<_>>();
    for file in files {
        let file_text = file.to_string_lossy().to_lowercase();
        let same_file = target_file
            .as_ref()
            .is_some_and(|target| file_text.ends_with(&target.to_lowercase()));
        let matches_terms = !terms.is_empty() && terms.iter().any(|term| file_text.contains(term));
        if (same_file || matches_terms) && seen.insert(file.clone()) {
            selected.push(file.clone());
        }
    }
    selected
}

fn prefilter_search_files(files: &[PathBuf], query: &str, _mode: &str) -> Vec<PathBuf> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }

    let selected = files
        .par_iter()
        .filter_map(|file| {
            let path_text = file.to_string_lossy().to_lowercase();
            if terms.iter().any(|term| path_text.contains(term)) {
                return Some(file.clone());
            }
            std::fs::read_to_string(file).ok().and_then(|source| {
                let source = source.to_lowercase();
                terms
                    .iter()
                    .any(|term| source.contains(term))
                    .then(|| file.clone())
            })
        })
        .collect::<Vec<_>>();
    dedup_paths(selected)
}

fn load_or_build_repo_structure_index(files: &[PathBuf], cwd: &Path) -> RepoStructureIndex {
    let fingerprint = format!("{:016x}", file_set_cache_key(files));
    let index_path = crate::storage::global_code_index_path();
    if let Ok(store) = CodeIndexStore::open(&index_path) {
        if let Ok(Some(index)) = store.load_repo_index(cwd, &fingerprint) {
            return index;
        }
    }

    let result = extract_files(files, cwd);
    let index = RepoStructureIndex::from_scan_result(&result);
    if let Ok(mut store) = CodeIndexStore::open(&index_path) {
        let _ = store.write_repo_index(cwd, &fingerprint, &index);
    }
    index
}

fn build_symbol_index(files: &[PathBuf], cwd: &Path) -> Vec<IndexedSymbol> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Vec<IndexedSymbol>>>> = OnceLock::new();
    let key = symbol_index_cache_key(files);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(index) = cache.get(&key) {
            return index.clone();
        }
    }

    let result = extract_files(files, cwd);
    let index = symbol_index_from_scan_result(&result);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, index.clone());
    }
    index
}

fn symbol_index_cache_key(files: &[PathBuf]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    files.len().hash(&mut hasher);
    for file in files {
        file.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(file) {
            meta.len().hash(&mut hasher);
            if let Ok(modified) = meta.modified() {
                if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                    duration.as_secs().hash(&mut hasher);
                    duration.subsec_nanos().hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

fn symbol_index_from_scan_result(result: &ScanResult) -> Vec<IndexedSymbol> {
    let mut symbols = Vec::new();
    for t in result.types.values() {
        symbols.push(IndexedSymbol {
            file: source_file(&t.source).to_string(),
            name: t.name.clone(),
            kind: format!("{:?}", t.kind).to_lowercase(),
            line: source_line(&t.source),
            text: format!(
                "{} {:?} {:?} {:?}",
                t.name, t.fields, t.variants, t.implements
            ),
            is_test: false,
        });
    }
    for f in result.functions.values() {
        symbols.push(IndexedSymbol {
            file: source_file(&f.source).to_string(),
            name: f.name.clone(),
            kind: "function".to_string(),
            line: source_line(&f.source),
            text: f.signature.clone(),
            is_test: f.is_test,
        });
    }
    symbols
}

fn repo_index_line(index: &[IndexedSymbol]) -> String {
    format!(
        "Repo intelligence: {} symbols, {} tests",
        index.len(),
        index.iter().filter(|symbol| symbol.is_test).count()
    )
}

fn repo_index_details(index: &[IndexedSymbol]) -> serde_json::Value {
    json!({
        "symbols": index.len(),
        "tests": index.iter().filter(|symbol| symbol.is_test).count(),
    })
}

fn repo_structure_index_line(index: &RepoStructureIndex) -> String {
    format!(
        "Repo intelligence: {} symbols, {} tests, {} edges",
        index.nodes.len(),
        index.nodes.iter().filter(|node| node.is_test).count(),
        index.edges.len()
    )
}

fn repo_structure_index_details(index: &RepoStructureIndex) -> serde_json::Value {
    json!({
        "symbols": index.nodes.len(),
        "tests": index.nodes.iter().filter(|node| node.is_test).count(),
        "edges": index.edges.len(),
    })
}

fn scan_symbol_kind_label(kind: crate::codeintel::SymbolKind) -> &'static str {
    match kind {
        crate::codeintel::SymbolKind::File => "file",
        crate::codeintel::SymbolKind::Module => "module",
        crate::codeintel::SymbolKind::Namespace => "namespace",
        crate::codeintel::SymbolKind::Package => "package",
        crate::codeintel::SymbolKind::Class => "class",
        crate::codeintel::SymbolKind::Struct => "struct",
        crate::codeintel::SymbolKind::Interface => "interface",
        crate::codeintel::SymbolKind::Enum => "enum",
        crate::codeintel::SymbolKind::Trait => "trait",
        crate::codeintel::SymbolKind::Function => "function",
        crate::codeintel::SymbolKind::Method => "method",
        crate::codeintel::SymbolKind::Constructor => "constructor",
        crate::codeintel::SymbolKind::Field => "field",
        crate::codeintel::SymbolKind::Property => "property",
        crate::codeintel::SymbolKind::Variable => "variable",
        crate::codeintel::SymbolKind::Constant => "constant",
        crate::codeintel::SymbolKind::TypeAlias => "type_alias",
        crate::codeintel::SymbolKind::Macro => "macro",
        crate::codeintel::SymbolKind::Unknown => "unknown",
    }
}

fn search_index(
    index: &[IndexedSymbol],
    query: &str,
    mode: &str,
    max_results: usize,
) -> Vec<SearchHit> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for symbol in index {
        let mut score = 0;
        let mut why = Vec::new();
        let path = symbol.file.to_lowercase();
        let name = symbol.name.to_lowercase();
        let text = symbol.text.to_lowercase();
        for term in &terms {
            if name == *term {
                score += 100;
                why.push(format!("symbol exactly matches {term}"));
            } else if name.contains(term) {
                score += 60;
                why.push(format!("symbol contains {term}"));
            }
            if mode != "symbol" {
                if path.contains(term) {
                    score += 25;
                    why.push(format!("path contains {term}"));
                }
                if text.contains(term) {
                    score += if mode == "concept" { 20 } else { 15 };
                    why.push(format!("signature/metadata contains {term}"));
                }
            }
        }
        if score > 0 {
            hits.push(SearchHit {
                file: symbol.file.clone(),
                symbol: Some(symbol.name.clone()),
                kind: symbol.kind.clone(),
                line: symbol.line,
                score,
                why,
            });
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    hits.truncate(max_results);
    hits
}

#[derive(Debug, Clone)]
struct TestHit {
    file: String,
    name: String,
    line: usize,
    command: Option<String>,
    why: String,
}

fn discover_tests(
    index: &[IndexedSymbol],
    target: &str,
    cwd: &Path,
    max_results: usize,
) -> Vec<TestHit> {
    let (target_file, target_symbol) = split_target(target);
    let target_terms = query_terms(target_symbol.as_deref().unwrap_or(target));
    let cargo = cwd.join("Cargo.toml").exists();
    let mut hits = Vec::new();
    for symbol in index
        .iter()
        .filter(|symbol| symbol.is_test || looks_like_test_file(&symbol.file))
    {
        let mut score = 0;
        if let Some(file) = &target_file {
            if symbol.file == *file {
                score += 50;
            } else if same_stem_or_test_neighbor(&symbol.file, file) {
                score += 35;
            }
        }
        for term in &target_terms {
            if symbol.name.to_lowercase().contains(term) {
                score += 20;
            }
        }
        if score > 0 || target_file.is_none() {
            let test_name = symbol.name.rsplit("::").next().unwrap_or(&symbol.name);
            hits.push((
                score,
                TestHit {
                    file: symbol.file.clone(),
                    name: symbol.name.clone(),
                    line: symbol.line,
                    command: if cargo && symbol.file.ends_with(".rs") {
                        Some(format!("cargo test {test_name}"))
                    } else {
                        None
                    },
                    why: if symbol.is_test {
                        "indexed test symbol"
                    } else {
                        "test file naming"
                    }
                    .to_string(),
                },
            ));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.file.cmp(&b.1.file)));
    hits.into_iter()
        .take(max_results)
        .map(|(_, hit)| hit)
        .collect()
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.to_lowercase())
        .collect()
}

fn source_line(source: &str) -> usize {
    source
        .rsplit_once(':')
        .and_then(|(_, line)| line.parse().ok())
        .unwrap_or(1)
}

fn split_target(target: &str) -> (Option<String>, Option<String>) {
    if let Some((file, symbol)) = target.split_once('#') {
        return (Some(file.to_string()), Some(symbol.to_string()));
    }
    if let Some((file, _line)) = target.rsplit_once(':') {
        return (Some(file.to_string()), None);
    }
    if target.contains('/') || target.contains('\\') {
        return (Some(target.to_string()), None);
    }
    (None, Some(target.to_string()))
}

fn looks_like_test_file(file: &str) -> bool {
    let name = Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file);
    name.contains("test")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.tsx")
        || name.ends_with("_test.go")
        || name.ends_with("_test.exs")
}

fn same_stem_or_test_neighbor(test_file: &str, target_file: &str) -> bool {
    let test_stem = Path::new(test_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .replace("_test", "")
        .replace(".test", "")
        .replace(".spec", "");
    let target_stem = Path::new(target_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    !target_stem.is_empty() && test_stem.contains(target_stem)
}

fn related_symbols<'a>(
    index: &'a [IndexedSymbol],
    target_file: Option<&str>,
    target_symbol: Option<&str>,
    max_results: usize,
) -> Vec<&'a IndexedSymbol> {
    let target_terms = target_symbol.map(query_terms).unwrap_or_default();
    let mut scored = Vec::new();
    for symbol in index {
        if target_symbol == Some(symbol.name.as_str()) {
            continue;
        }
        let mut score = 0;
        if target_file == Some(symbol.file.as_str()) {
            score += 50;
        }
        for term in &target_terms {
            if symbol.name.to_lowercase().contains(term)
                || symbol.text.to_lowercase().contains(term)
            {
                score += 15;
            }
        }
        if score > 0 {
            scored.push((score, symbol));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.line.cmp(&b.1.line)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, symbol)| symbol)
        .collect()
}

fn execute_related(mut files: Vec<PathBuf>, cwd: &Path, target: &str) -> ToolOutput {
    files.sort();
    files.dedup();
    let index_files = prefilter_related_files(&files, target);
    let index = build_symbol_index(&index_files, cwd);
    let (target_file, target_symbol) = split_target(target);
    let related = related_symbols(&index, target_file.as_deref(), target_symbol.as_deref(), 12);
    let tests = discover_tests(&index, target, cwd, 5);
    let definition = target_symbol.as_ref().and_then(|name| {
        index.iter().find(|symbol| {
            symbol.name == *name && target_file.as_ref().is_none_or(|file| symbol.file == *file)
        })
    });

    let mut lines = vec![
        format!("Action: related"),
        format!("Target: {target}"),
        format!("Files analyzed: {}", files.len()),
        repo_index_line(&index),
    ];
    if let Some(symbol) = definition {
        lines.push(format!(
            "Definition: {}:{} [{}] {} (extract: {}#{})",
            symbol.file, symbol.line, symbol.kind, symbol.name, symbol.file, symbol.name
        ));
    }
    if !related.is_empty() {
        lines.push("Related symbols:".to_string());
        for symbol in &related {
            lines.push(format!(
                "- {}:{} [{}] {} (extract: {}#{})",
                symbol.file, symbol.line, symbol.kind, symbol.name, symbol.file, symbol.name
            ));
        }
    }
    if !tests.is_empty() {
        lines.push("Likely tests:".to_string());
        for test in &tests {
            lines.push(format!(
                "- {}:{} {} — {}",
                test.file,
                test.line,
                test.name,
                test.command.as_deref().unwrap_or("no command inferred")
            ));
        }
    }
    if definition.is_none() && related.is_empty() && tests.is_empty() {
        lines.push("No related context found.".to_string());
    }

    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: truncate_output(lines.join("\n")),
        }],
        details: json!({
            "action": "related",
            "target": target,
            "files_analyzed": files.len(),
            "repo_intelligence": repo_index_details(&index),
            "definition": definition.map(|symbol| json!({
                "file": symbol.file,
                "symbol": symbol.name,
                "kind": symbol.kind,
                "line": symbol.line,
                "extract_target": format!("{}#{}", symbol.file, symbol.name),
            })),
            "related": related.iter().map(|symbol| json!({
                "file": symbol.file,
                "symbol": symbol.name,
                "kind": symbol.kind,
                "line": symbol.line,
                "extract_target": format!("{}#{}", symbol.file, symbol.name),
            })).collect::<Vec<_>>(),
            "tests": tests.iter().map(|test| json!({
                "file": test.file,
                "symbol": test.name,
                "line": test.line,
                "command": test.command,
                "why": test.why,
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    }
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

enum Locator {
    Line(usize),
    Range(usize, usize),
    Symbol(String),
}

fn execute_extract(targets: &[String], ctx: &ToolContext) -> ToolOutput {
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

fn parse_extract_target(target: &str) -> Option<(String, Locator)> {
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

#[cfg(test)]
fn get_parser(path: &Path) -> Option<tree_sitter::Parser> {
    crate::tools::code_intel::parser_for_path(path)
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

#[cfg(test)]
mod tests;
