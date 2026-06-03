use super::{
    AdapterDescriptor, AdapterSource, BackendProvenance, CodeIntelAdapter, CodeIntelEnvelope,
    CodeIntelQuery, CodeIntelSurface, DefinitionItem, Freshness, Location, Position, ReferenceItem,
    ReferenceUsage, RelatedLocation, RelationshipKind, SymbolIdentity, SymbolKind, TextRange,
    Truncation,
};

const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIndexSnapshot {
    pub workspace_root: Option<String>,
    pub freshness: Freshness,
    pub nodes: Vec<RepoIndexNode>,
    pub edges: Vec<RepoIndexEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIndexNode {
    pub id: String,
    pub symbol: SymbolIdentity,
    pub location: Location,
    pub searchable_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIndexEdge {
    pub from: String,
    pub to: String,
    pub relationship: RelationshipKind,
    pub label: Option<String>,
    pub usage: ReferenceUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralContextItem {
    pub symbol: SymbolIdentity,
    pub location: Location,
    pub related: Vec<RelatedLocation>,
}

pub struct RepoIndexCodeIntelAdapter {
    descriptor: AdapterDescriptor,
    snapshot: RepoIndexSnapshot,
}

impl RepoIndexCodeIntelAdapter {
    pub fn new(snapshot: RepoIndexSnapshot) -> Self {
        Self {
            descriptor: AdapterDescriptor {
                id: "repo-index".to_string(),
                display_name: "Repo index".to_string(),
                languages: vec!["rust".to_string()],
                surfaces: vec![
                    CodeIntelSurface::WorkspaceSymbols,
                    CodeIntelSurface::DocumentSymbols,
                    CodeIntelSurface::StructuralContext,
                    CodeIntelSurface::Definition,
                    CodeIntelSurface::References,
                ],
                source: AdapterSource::BuiltIn,
            },
            snapshot,
        }
    }

    pub fn workspace_symbols(&self, query: &str) -> CodeIntelEnvelope<DefinitionItem> {
        let query_lower = query.to_lowercase();
        let mut items = self
            .snapshot
            .nodes
            .iter()
            .filter(|node| {
                query_lower.is_empty()
                    || node.symbol.name.to_lowercase().contains(&query_lower)
                    || node
                        .symbol
                        .container
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query_lower)
                    || node.searchable_text.to_lowercase().contains(&query_lower)
            })
            .map(|node| DefinitionItem {
                symbol: Some(node.symbol.clone()),
                target: node.location.clone(),
                provenance: Some("repo-index best-effort workspace symbol".to_string()),
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            let a = a.symbol.as_ref().map(|s| s.name.as_str()).unwrap_or("");
            let b = b.symbol.as_ref().map(|s| s.name.as_str()).unwrap_or("");
            a.cmp(b)
        });
        let available = items.len();
        items.truncate(DEFAULT_LIMIT);
        self.envelope(
            CodeIntelSurface::WorkspaceSymbols,
            CodeIntelQuery::WorkspaceSymbols {
                query: query.to_string(),
            },
            format!("{} workspace symbol{}", items.len(), plural(items.len())),
            items,
            truncation(items_len_for_summary(available, DEFAULT_LIMIT), available),
            Vec::new(),
        )
    }

    pub fn document_symbols(&self, path: &str) -> CodeIntelEnvelope<super::DocumentSymbolItem> {
        let mut items = self
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.location.path == path)
            .map(|node| super::DocumentSymbolItem {
                symbol: node.symbol.clone(),
                location: node.location.clone(),
                children: Vec::new(),
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| line(&a.location).cmp(&line(&b.location)));
        let available = items.len();
        items.truncate(DEFAULT_LIMIT);
        self.envelope(
            CodeIntelSurface::DocumentSymbols,
            CodeIntelQuery::DocumentSymbols {
                path: path.to_string(),
            },
            format!("{} document symbol{}", items.len(), plural(items.len())),
            items,
            truncation(items_len_for_summary(available, DEFAULT_LIMIT), available),
            Vec::new(),
        )
    }

    pub fn structural_context(
        &self,
        path: &str,
        position: Option<Position>,
    ) -> CodeIntelEnvelope<StructuralContextItem> {
        let target = self.node_at(path, position).or_else(|| {
            self.snapshot
                .nodes
                .iter()
                .find(|node| node.location.path == path)
        });
        let items = target
            .map(|node| StructuralContextItem {
                symbol: node.symbol.clone(),
                location: node.location.clone(),
                related: self.related_locations(&node.id),
            })
            .into_iter()
            .collect::<Vec<_>>();
        let warnings = if items.is_empty() {
            vec!["repo index did not resolve structural context for this location".to_string()]
        } else {
            Vec::new()
        };
        self.envelope(
            CodeIntelSurface::StructuralContext,
            CodeIntelQuery::StructuralContext {
                path: path.to_string(),
                position,
            },
            format!(
                "{} structural context item{}",
                items.len(),
                plural(items.len())
            ),
            items,
            None,
            warnings,
        )
    }

    pub fn definition(&self, path: &str, position: Position) -> CodeIntelEnvelope<DefinitionItem> {
        let items = self
            .node_at(path, Some(position))
            .map(|node| DefinitionItem {
                symbol: Some(node.symbol.clone()),
                target: node.location.clone(),
                provenance: Some("repo-index exact node match".to_string()),
            })
            .into_iter()
            .collect::<Vec<_>>();
        let warnings = if items.is_empty() {
            vec![
                "repo index could not provide a best-effort definition; try StructuralContext"
                    .to_string(),
            ]
        } else {
            Vec::new()
        };
        self.envelope(
            CodeIntelSurface::Definition,
            CodeIntelQuery::Definition {
                path: path.to_string(),
                position,
            },
            format!("{} definition{}", items.len(), plural(items.len())),
            items,
            None,
            warnings,
        )
    }

    pub fn references(&self, path: &str, position: Position) -> CodeIntelEnvelope<ReferenceItem> {
        let Some(node) = self.node_at(path, Some(position)) else {
            return self.envelope(
                CodeIntelSurface::References,
                CodeIntelQuery::References {
                    path: path.to_string(),
                    position,
                },
                "0 references".to_string(),
                Vec::new(),
                None,
                vec![
                    "repo index could not resolve the reference target; try StructuralContext"
                        .to_string(),
                ],
            );
        };
        let items = self
            .snapshot
            .edges
            .iter()
            .filter(|edge| edge.to == node.id || edge.from == node.id)
            .filter_map(|edge| {
                let other = if edge.to == node.id {
                    &edge.from
                } else {
                    &edge.to
                };
                self.node_by_id(other).map(|related| ReferenceItem {
                    symbol: Some(related.symbol.clone()),
                    location: related.location.clone(),
                    usage: edge.usage,
                })
            })
            .collect::<Vec<_>>();
        self.envelope(
            CodeIntelSurface::References,
            CodeIntelQuery::References {
                path: path.to_string(),
                position,
            },
            format!(
                "{} best-effort reference{}",
                items.len(),
                plural(items.len())
            ),
            items,
            None,
            vec!["references are best-effort repo-index relationships, not LSP-exact".to_string()],
        )
    }

    fn envelope<T>(
        &self,
        surface: CodeIntelSurface,
        query: CodeIntelQuery,
        summary: String,
        items: Vec<T>,
        truncated: Option<Truncation>,
        warnings: Vec<String>,
    ) -> CodeIntelEnvelope<T> {
        CodeIntelEnvelope {
            surface,
            query,
            summary,
            items,
            provenance: BackendProvenance {
                adapter: self.descriptor.id.clone(),
                backend: "repo-index".to_string(),
                language: None,
                workspace_root: self.snapshot.workspace_root.clone(),
                freshness: self.snapshot.freshness.clone(),
            },
            truncated,
            warnings,
            suggested_next_queries: Vec::new(),
        }
    }

    fn node_at(&self, path: &str, position: Option<Position>) -> Option<&RepoIndexNode> {
        let mut candidates = self
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.location.path == path)
            .collect::<Vec<_>>();
        if let Some(position) = position {
            candidates.retain(|node| contains_position(&node.location, position));
        }
        candidates.sort_by(|a, b| line(&b.location).cmp(&line(&a.location)));
        candidates.into_iter().next()
    }

    fn node_by_id(&self, id: &str) -> Option<&RepoIndexNode> {
        self.snapshot.nodes.iter().find(|node| node.id == id)
    }

    fn related_locations(&self, id: &str) -> Vec<RelatedLocation> {
        self.snapshot
            .edges
            .iter()
            .filter_map(|edge| {
                let related_id = if edge.from == id {
                    &edge.to
                } else if edge.to == id {
                    &edge.from
                } else {
                    return None;
                };
                self.node_by_id(related_id).map(|node| RelatedLocation {
                    location: node.location.clone(),
                    relationship: edge.relationship,
                    label: edge.label.clone(),
                })
            })
            .collect()
    }
}

impl CodeIntelAdapter for RepoIndexCodeIntelAdapter {
    fn descriptor(&self) -> &AdapterDescriptor {
        &self.descriptor
    }
}

fn contains_position(location: &Location, position: Position) -> bool {
    let Some(range) = &location.range else {
        return true;
    };
    range.start.line <= position.line && position.line <= range.end.line
}

fn line(location: &Location) -> u32 {
    location
        .range
        .as_ref()
        .map(|range| range.start.line)
        .unwrap_or(0)
}

fn truncation(returned: usize, available: usize) -> Option<Truncation> {
    (available > returned).then(|| Truncation {
        returned,
        available: Some(available),
        reason: "repo-index result limit".to_string(),
        continuation: None,
    })
}

fn items_len_for_summary(available: usize, limit: usize) -> usize {
    available.min(limit)
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> Option<TextRange> {
        Some(TextRange {
            start: Position {
                line: start,
                column: 0,
            },
            end: Position {
                line: end,
                column: 0,
            },
        })
    }

    fn node(
        id: &str,
        name: &str,
        kind: SymbolKind,
        path: &str,
        start: u32,
        end: u32,
    ) -> RepoIndexNode {
        RepoIndexNode {
            id: id.to_string(),
            symbol: SymbolIdentity {
                name: name.to_string(),
                kind,
                container: Some("crate::module".to_string()),
                language: Some("rust".to_string()),
            },
            location: Location {
                path: path.to_string(),
                range: range(start, end),
            },
            searchable_text: format!("crate::module::{name}"),
        }
    }

    fn adapter() -> RepoIndexCodeIntelAdapter {
        RepoIndexCodeIntelAdapter::new(RepoIndexSnapshot {
            workspace_root: Some("/repo".to_string()),
            freshness: Freshness::Fresh,
            nodes: vec![
                node(
                    "type:Widget",
                    "Widget",
                    SymbolKind::Struct,
                    "src/lib.rs",
                    1,
                    20,
                ),
                node(
                    "fn:build_widget",
                    "build_widget",
                    SymbolKind::Function,
                    "src/lib.rs",
                    5,
                    8,
                ),
                node(
                    "fn:test_widget",
                    "test_widget",
                    SymbolKind::Function,
                    "tests/widget.rs",
                    3,
                    5,
                ),
            ],
            edges: vec![RepoIndexEdge {
                from: "fn:test_widget".to_string(),
                to: "fn:build_widget".to_string(),
                relationship: RelationshipKind::Reference,
                label: Some("test references function".to_string()),
                usage: ReferenceUsage::Call,
            }],
        })
    }

    #[test]
    fn codeintel_adapter_descriptor_exposes_repo_index_surfaces() {
        let adapter = adapter();
        let descriptor = adapter.descriptor();
        assert_eq!(descriptor.id, "repo-index");
        assert!(descriptor
            .surfaces
            .contains(&CodeIntelSurface::WorkspaceSymbols));
        assert!(descriptor
            .surfaces
            .contains(&CodeIntelSurface::StructuralContext));
        assert!(descriptor.surfaces.contains(&CodeIntelSurface::Definition));
    }

    #[test]
    fn codeintel_symbol_workspace_and_document_symbols_are_bounded_and_deterministic() {
        let adapter = adapter();
        let workspace = adapter.workspace_symbols("widget");
        assert_eq!(workspace.provenance.backend, "repo-index");
        assert!(workspace.items.len() >= 2);
        assert_eq!(workspace.items[0].symbol.as_ref().unwrap().name, "Widget");

        let document = adapter.document_symbols("src/lib.rs");
        assert_eq!(document.items.len(), 2);
        assert_eq!(document.items[0].symbol.name, "Widget");
        assert_eq!(document.items[1].symbol.name, "build_widget");
    }

    #[test]
    fn codeintel_structural_context_returns_related_locations() {
        let adapter = adapter();
        let context =
            adapter.structural_context("src/lib.rs", Some(Position { line: 6, column: 0 }));
        assert_eq!(context.items.len(), 1);
        assert_eq!(context.items[0].symbol.name, "build_widget");
        assert_eq!(context.items[0].related.len(), 1);
        assert_eq!(
            context.items[0].related[0].relationship,
            RelationshipKind::Reference
        );
    }

    #[test]
    fn codeintel_adapter_definition_and_references_are_best_effort() {
        let adapter = adapter();
        let definition = adapter.definition("src/lib.rs", Position { line: 6, column: 0 });
        assert_eq!(definition.items.len(), 1);
        assert_eq!(
            definition.items[0].symbol.as_ref().unwrap().name,
            "build_widget"
        );

        let references = adapter.references("src/lib.rs", Position { line: 6, column: 0 });
        assert_eq!(references.items.len(), 1);
        assert_eq!(references.items[0].usage, ReferenceUsage::Call);
        assert!(!references.warnings.is_empty());
    }
}
