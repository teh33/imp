use super::*;

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

pub(super) fn execute_search(
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

pub(super) fn execute_tests(
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

pub(super) fn symbol_index_cache_key(files: &[PathBuf]) -> u64 {
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

pub(super) fn execute_related(mut files: Vec<PathBuf>, cwd: &Path, target: &str) -> ToolOutput {
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
