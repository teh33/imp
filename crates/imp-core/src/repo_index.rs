use crate::codeintel::{Freshness, Location, Position, SymbolIdentity, SymbolKind, TextRange};
use crate::tools::scan::types::{EdgeKind, ScanResult, TypeKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoNodeId(String);

impl RepoNodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoStructureIndex {
    pub workspace_root: Option<String>,
    pub freshness: Freshness,
    pub nodes: Vec<RepoNode>,
    pub edges: Vec<RepoEdge>,
}

impl RepoStructureIndex {
    pub fn from_scan_result(result: &ScanResult) -> Self {
        let mut builder = RepoIndexBuilder::default();
        builder.add_scan_result(result);
        builder.finish()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<RepoSearchHit> {
        let terms = tokenize_identifier_query(query);
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }
        let wants_tests = terms
            .iter()
            .any(|term| matches!(term.as_str(), "test" | "tests"));
        let mut hits = self
            .nodes
            .iter()
            .filter_map(|node| {
                let mut score = 0_i32;
                let mut why = Vec::new();
                let name = node.symbol.name.to_lowercase();
                let qualified = node.qualified_name.to_lowercase();
                let path = node.location.path.to_lowercase();
                let text = node.searchable_text.to_lowercase();
                for term in &terms {
                    if name == *term {
                        score += 120;
                        why.push(format!("name exactly matches {term}"));
                    } else if name.contains(term) {
                        score += 70;
                        why.push(format!("name contains {term}"));
                    }
                    if qualified != name && qualified.contains(term) {
                        score += 45;
                        why.push(format!("qualified name contains {term}"));
                    }
                    if path.contains(term) {
                        score += 25;
                        why.push(format!("path contains {term}"));
                    }
                    if text.contains(term) {
                        score += 15;
                        why.push(format!("metadata contains {term}"));
                    }
                }
                if score == 0 {
                    return None;
                }
                if node.is_test && !wants_tests {
                    score = score * 3 / 5;
                    why.push("test result penalty".to_string());
                }
                Some(RepoSearchHit {
                    node: node.clone(),
                    score,
                    why,
                })
            })
            .collect::<Vec<_>>();

        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.node.location.path.cmp(&b.node.location.path))
                .then_with(|| node_line(&a.node).cmp(&node_line(&b.node)))
        });

        let mut per_file = HashMap::<String, usize>::new();
        for hit in &mut hits {
            let count = per_file.entry(hit.node.location.path.clone()).or_default();
            if *count >= 2 {
                hit.score /= 2;
                hit.why.push("same-file saturation penalty".to_string());
            }
            *count += 1;
        }
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.node.location.path.cmp(&b.node.location.path))
                .then_with(|| node_line(&a.node).cmp(&node_line(&b.node)))
        });
        hits.truncate(limit);
        hits
    }

    pub fn related(&self, target: &str, limit: usize) -> Vec<RepoRelatedHit> {
        if limit == 0 {
            return Vec::new();
        }
        let Some(target_node) = self.resolve_target(target) else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        let mut seen = HashSet::new();
        seen.insert(target_node.id.clone());

        for edge in &self.edges {
            let related_id = match &edge.target {
                RepoEdgeTarget::Node(id) if edge.from == target_node.id => Some(id),
                RepoEdgeTarget::Node(id) if *id == target_node.id => Some(&edge.from),
                _ => None,
            };
            if let Some(id) = related_id {
                if seen.insert(id.clone()) {
                    if let Some(node) = self.node_by_id(id) {
                        hits.push(RepoRelatedHit {
                            node: node.clone(),
                            relationship: edge.kind,
                            why: edge
                                .label
                                .clone()
                                .unwrap_or_else(|| edge.kind.label().to_string()),
                        });
                    }
                }
            }
        }

        for node in self
            .nodes
            .iter()
            .filter(|node| node.location.path == target_node.location.path)
        {
            if seen.insert(node.id.clone()) {
                hits.push(RepoRelatedHit {
                    node: node.clone(),
                    relationship: RepoRelationshipKind::SameFile,
                    why: "same file".to_string(),
                });
            }
            if hits.len() >= limit {
                break;
            }
        }
        hits.truncate(limit);
        hits
    }

    pub fn node_at(&self, path: &str, position: Option<Position>) -> Option<&RepoNode> {
        let mut candidates = self
            .nodes
            .iter()
            .filter(|node| node.location.path == path)
            .collect::<Vec<_>>();
        if let Some(position) = position {
            candidates.retain(|node| contains_position(&node.location, position));
        }
        candidates.sort_by(|a, b| node_line(b).cmp(&node_line(a)));
        candidates.into_iter().next()
    }

    pub fn node_by_id(&self, id: &RepoNodeId) -> Option<&RepoNode> {
        self.nodes.iter().find(|node| node.id == *id)
    }

    pub fn resolve_target_node(&self, target: &str) -> Option<&RepoNode> {
        self.resolve_target(target)
    }

    fn resolve_target(&self, target: &str) -> Option<&RepoNode> {
        if let Some((path, symbol)) = target.split_once('#') {
            return self.nodes.iter().find(|node| {
                node.location.path == path
                    && (node.symbol.name == symbol || node.qualified_name.ends_with(symbol))
            });
        }
        self.nodes
            .iter()
            .find(|node| node.qualified_name == target || node.symbol.name == target)
            .or_else(|| {
                self.nodes
                    .iter()
                    .find(|node| node.qualified_name.ends_with(target))
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoNode {
    pub id: RepoNodeId,
    pub symbol: SymbolIdentity,
    pub qualified_name: String,
    pub location: Location,
    pub searchable_text: String,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEdge {
    pub from: RepoNodeId,
    pub target: RepoEdgeTarget,
    pub kind: RepoRelationshipKind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoEdgeTarget {
    Node(RepoNodeId),
    Symbol(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRelationshipKind {
    Contains,
    Implements,
    Imports,
    Calls,
    Tests,
    SameFile,
}

impl RepoRelationshipKind {
    fn label(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Implements => "implements",
            Self::Imports => "imports",
            Self::Calls => "calls",
            Self::Tests => "tests",
            Self::SameFile => "same file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSearchHit {
    pub node: RepoNode,
    pub score: i32,
    pub why: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRelatedHit {
    pub node: RepoNode,
    pub relationship: RepoRelationshipKind,
    pub why: String,
}

#[derive(Default)]
struct RepoIndexBuilder {
    nodes: Vec<RepoNode>,
    edges: Vec<RepoEdge>,
    by_name: HashMap<String, RepoNodeId>,
}

impl RepoIndexBuilder {
    fn add_scan_result(&mut self, result: &ScanResult) {
        for ty in result.types.values() {
            let id = stable_node_id(&ty.source, &ty.name, "type");
            let node = RepoNode {
                id: id.clone(),
                symbol: SymbolIdentity {
                    name: ty.name.clone(),
                    kind: symbol_kind_for_type(&ty.kind),
                    container: None,
                    language: language_for_source(&ty.source),
                },
                qualified_name: ty.name.clone(),
                location: location_for_source(&ty.source),
                searchable_text: format!(
                    "{} {:?} {:?} {:?}",
                    ty.name, ty.fields, ty.variants, ty.implements
                ),
                is_test: false,
            };
            self.by_name.insert(ty.name.clone(), id);
            self.nodes.push(node);
        }

        for function in result.functions.values() {
            let id = stable_node_id(&function.source, &function.name, "function");
            let container = function
                .name
                .rsplit_once("::")
                .map(|(container, _)| container.to_string());
            let node = RepoNode {
                id: id.clone(),
                symbol: SymbolIdentity {
                    name: function.name.clone(),
                    kind: SymbolKind::Function,
                    container,
                    language: language_for_source(&function.source),
                },
                qualified_name: function.name.clone(),
                location: location_for_source(&function.source),
                searchable_text: function.signature.clone(),
                is_test: function.is_test,
            };
            self.by_name.insert(function.name.clone(), id);
            self.nodes.push(node);
        }

        for edge in &result.edges {
            let Some(from) = self.by_name.get(&edge.from).cloned() else {
                continue;
            };
            let target = self
                .by_name
                .get(&edge.to)
                .cloned()
                .map(RepoEdgeTarget::Node)
                .unwrap_or_else(|| RepoEdgeTarget::Symbol(edge.to.clone()));
            self.edges.push(RepoEdge {
                from,
                target,
                kind: relationship_kind(edge.kind),
                label: edge.label.clone(),
            });
        }
    }

    fn finish(self) -> RepoStructureIndex {
        RepoStructureIndex {
            workspace_root: None,
            freshness: Freshness::Fresh,
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

pub fn tokenize_identifier_query(query: &str) -> Vec<String> {
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    let token_re =
        TOKEN_RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid token regex"));
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    for raw in token_re.find_iter(query).map(|m| m.as_str()) {
        let lower = raw.to_lowercase();
        push_unique(&mut terms, &mut seen, lower.clone());
        if raw.contains('_') {
            for part in lower.split('_').filter(|part| !part.is_empty()) {
                push_unique(&mut terms, &mut seen, part.to_string());
            }
        } else {
            for part in split_camel_identifier(raw) {
                push_unique(&mut terms, &mut seen, part);
            }
        }
    }
    terms
}

fn split_camel_identifier(raw: &str) -> Vec<String> {
    let chars = raw.char_indices().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0;
    for idx in 1..chars.len() {
        let (_, prev) = chars[idx - 1];
        let (byte_idx, current) = chars[idx];
        let next = chars.get(idx + 1).map(|(_, c)| *c);
        let boundary = current.is_ascii_digit() != prev.is_ascii_digit()
            || (current.is_ascii_uppercase() && prev.is_ascii_lowercase())
            || (prev.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|next| next.is_ascii_lowercase()));
        if boundary {
            parts.push(raw[start..byte_idx].to_lowercase());
            start = byte_idx;
        }
    }
    parts.push(raw[start..].to_lowercase());
    parts
}

fn push_unique(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    if seen.insert(term.clone()) {
        terms.push(term);
    }
}

fn stable_node_id(source: &str, name: &str, kind: &str) -> RepoNodeId {
    RepoNodeId::new(format!("{source}#{kind}:{name}"))
}

fn symbol_kind_for_type(kind: &TypeKind) -> SymbolKind {
    match kind {
        TypeKind::Struct => SymbolKind::Struct,
        TypeKind::Class => SymbolKind::Class,
        TypeKind::Interface => SymbolKind::Interface,
        TypeKind::Enum => SymbolKind::Enum,
        TypeKind::Trait => SymbolKind::Trait,
        TypeKind::TypeAlias => SymbolKind::TypeAlias,
        TypeKind::Union => SymbolKind::Unknown,
        TypeKind::Protocol => SymbolKind::Interface,
    }
}

fn relationship_kind(kind: EdgeKind) -> RepoRelationshipKind {
    match kind {
        EdgeKind::Contains => RepoRelationshipKind::Contains,
        EdgeKind::Implements => RepoRelationshipKind::Implements,
        EdgeKind::Imports => RepoRelationshipKind::Imports,
        EdgeKind::Calls => RepoRelationshipKind::Calls,
        EdgeKind::Tests => RepoRelationshipKind::Tests,
    }
}

fn location_for_source(source: &str) -> Location {
    Location {
        path: source_file(source),
        range: Some(TextRange {
            start: Position {
                line: source_line(source),
                column: 0,
            },
            end: Position {
                line: source_line(source),
                column: 0,
            },
        }),
    }
}

fn source_file(source: &str) -> String {
    source
        .rsplit_once(':')
        .map(|(file, _)| file)
        .unwrap_or(source)
        .to_string()
}

fn source_line(source: &str) -> u32 {
    source
        .rsplit_once(':')
        .and_then(|(_, line)| line.parse().ok())
        .unwrap_or(1)
}

fn language_for_source(source: &str) -> Option<String> {
    let file = source_file(source);
    Path::new(&file)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_string())
}

fn node_line(node: &RepoNode) -> u32 {
    node.location
        .range
        .as_ref()
        .map(|range| range.start.line)
        .unwrap_or(0)
}

fn contains_position(location: &Location, position: Position) -> bool {
    let Some(range) = &location.range else {
        return true;
    };
    range.start.line <= position.line && position.line <= range.end.line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::scan::types::{EdgeInfo, FunctionInfo, ScanResult, TypeInfo};

    #[test]
    fn scan_search_repo_index_tokenizes_identifier_parts() {
        assert_eq!(
            tokenize_identifier_query("getHTTPResponse_handler"),
            vec!["gethttpresponse_handler", "gethttpresponse", "handler"]
        );
        assert_eq!(
            tokenize_identifier_query("HTTPResponse"),
            vec!["httpresponse", "http", "response"]
        );
    }

    #[test]
    fn scan_search_repo_index_ranks_symbols_from_scan_result() {
        let mut result = ScanResult::default();
        result.functions.insert(
            "execute_search".to_string(),
            FunctionInfo {
                name: "execute_search".to_string(),
                source: "src/tools/scan/mod.rs:42".to_string(),
                signature: "fn execute_search(query: &str)".to_string(),
                ..FunctionInfo::default()
            },
        );
        result.types.insert(
            "SearchState".to_string(),
            TypeInfo {
                name: "SearchState".to_string(),
                source: "src/tools/scan/mod.rs:10".to_string(),
                ..TypeInfo::default()
            },
        );
        let index = RepoStructureIndex::from_scan_result(&result);
        let hits = index.search("execute search", 5);
        assert_eq!(hits[0].node.symbol.name, "execute_search");
        assert!(hits[0].why.iter().any(|why| why.contains("name contains")));
    }

    #[test]
    fn scan_related_repo_index_uses_resolved_edges_before_same_file() {
        let mut result = ScanResult::default();
        result.functions.insert(
            "build_widget".to_string(),
            FunctionInfo {
                name: "build_widget".to_string(),
                source: "src/lib.rs:5".to_string(),
                signature: "fn build_widget()".to_string(),
                ..FunctionInfo::default()
            },
        );
        result.functions.insert(
            "test_widget".to_string(),
            FunctionInfo {
                name: "test_widget".to_string(),
                source: "tests/widget.rs:3".to_string(),
                signature: "fn test_widget()".to_string(),
                is_test: true,
                ..FunctionInfo::default()
            },
        );
        result.edges.push(EdgeInfo {
            from: "test_widget".to_string(),
            to: "build_widget".to_string(),
            source: "tests/widget.rs:4".to_string(),
            kind: EdgeKind::Calls,
            label: Some("test calls function".to_string()),
        });
        let index = RepoStructureIndex::from_scan_result(&result);
        let related = index.related("build_widget", 5);
        assert_eq!(related[0].node.symbol.name, "test_widget");
        assert_eq!(related[0].relationship, RepoRelationshipKind::Calls);
    }
}
