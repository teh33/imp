use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::codeintel::{Freshness, Location, Position, SymbolIdentity, SymbolKind, TextRange};
use crate::repo_index::{
    RepoEdge, RepoEdgeTarget, RepoNode, RepoNodeId, RepoRelationshipKind, RepoStructureIndex,
};
use crate::{Error, Result};

const SCHEMA_VERSION: i64 = 1;

pub struct CodeIndexStore {
    conn: Connection,
}

impl CodeIndexStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let conn = Connection::open(path).map_err(sql_error)?;
        let store = Self { conn };
        store.initialize()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            conn: Connection::open_in_memory().map_err(sql_error)?,
        };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS repos (
                    id INTEGER PRIMARY KEY,
                    root TEXT NOT NULL UNIQUE,
                    fingerprint TEXT NOT NULL,
                    schema_version INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS files (
                    id INTEGER PRIMARY KEY,
                    repo_id INTEGER NOT NULL,
                    path TEXT NOT NULL,
                    language TEXT,
                    content_hash TEXT NOT NULL,
                    modified_at INTEGER,
                    size INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL,
                    symbol_count INTEGER NOT NULL,
                    edge_count INTEGER NOT NULL,
                    parse_error TEXT,
                    UNIQUE(repo_id, path),
                    FOREIGN KEY(repo_id) REFERENCES repos(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS symbols (
                    id INTEGER PRIMARY KEY,
                    repo_id INTEGER NOT NULL,
                    file_id INTEGER NOT NULL,
                    stable_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    qualified_name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    line_start INTEGER NOT NULL,
                    line_end INTEGER,
                    column_start INTEGER,
                    column_end INTEGER,
                    searchable_text TEXT NOT NULL,
                    is_test INTEGER NOT NULL,
                    UNIQUE(repo_id, stable_id),
                    FOREIGN KEY(repo_id) REFERENCES repos(id) ON DELETE CASCADE,
                    FOREIGN KEY(file_id) REFERENCES files(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS edges (
                    id INTEGER PRIMARY KEY,
                    repo_id INTEGER NOT NULL,
                    from_stable_id TEXT NOT NULL,
                    to_stable_id TEXT,
                    to_name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    label TEXT,
                    FOREIGN KEY(repo_id) REFERENCES repos(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_symbols_repo_name ON symbols(repo_id, name);
                CREATE INDEX IF NOT EXISTS idx_symbols_repo_qualified ON symbols(repo_id, qualified_name);
                CREATE INDEX IF NOT EXISTS idx_symbols_repo_file ON symbols(repo_id, file_id);
                CREATE INDEX IF NOT EXISTS idx_symbols_repo_test ON symbols(repo_id, is_test);
                CREATE INDEX IF NOT EXISTS idx_edges_repo_from ON edges(repo_id, from_stable_id);
                CREATE INDEX IF NOT EXISTS idx_edges_repo_to ON edges(repo_id, to_stable_id);
                CREATE INDEX IF NOT EXISTS idx_files_repo_path ON files(repo_id, path);
                "#,
            )
            .map_err(sql_error)
    }

    pub fn write_repo_index(
        &mut self,
        root: &Path,
        fingerprint: &str,
        index: &RepoStructureIndex,
    ) -> Result<()> {
        let root = root.to_string_lossy().to_string();
        let tx = self.conn.transaction().map_err(sql_error)?;
        tx.execute(
            "INSERT INTO repos(root, fingerprint, schema_version, indexed_at) VALUES(?1, ?2, ?3, strftime('%s','now'))
             ON CONFLICT(root) DO UPDATE SET fingerprint=excluded.fingerprint, schema_version=excluded.schema_version, indexed_at=excluded.indexed_at",
            params![root, fingerprint, SCHEMA_VERSION],
        )
        .map_err(sql_error)?;
        let repo_id: i64 = tx
            .query_row(
                "SELECT id FROM repos WHERE root = ?1",
                params![root],
                |row| row.get(0),
            )
            .map_err(sql_error)?;

        tx.execute("DELETE FROM edges WHERE repo_id = ?1", params![repo_id])
            .map_err(sql_error)?;
        tx.execute("DELETE FROM symbols WHERE repo_id = ?1", params![repo_id])
            .map_err(sql_error)?;
        tx.execute("DELETE FROM files WHERE repo_id = ?1", params![repo_id])
            .map_err(sql_error)?;

        for node in &index.nodes {
            let file_id = ensure_file(
                &tx,
                repo_id,
                &node.location.path,
                node.symbol.language.as_deref(),
            )?;
            let (line_start, line_end, column_start, column_end) = range_parts(&node.location);
            tx.execute(
                "INSERT INTO symbols(repo_id, file_id, stable_id, name, qualified_name, kind, line_start, line_end, column_start, column_end, searchable_text, is_test)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    repo_id,
                    file_id,
                    node.id.as_str(),
                    node.symbol.name,
                    node.qualified_name,
                    symbol_kind_to_str(node.symbol.kind),
                    line_start,
                    line_end,
                    column_start,
                    column_end,
                    node.searchable_text,
                    if node.is_test { 1 } else { 0 },
                ],
            )
            .map_err(sql_error)?;
        }

        for edge in &index.edges {
            let (to_stable_id, to_name) = match &edge.target {
                RepoEdgeTarget::Node(id) => {
                    (Some(id.as_str().to_string()), id.as_str().to_string())
                }
                RepoEdgeTarget::Symbol(name) => (None, name.clone()),
            };
            tx.execute(
                "INSERT INTO edges(repo_id, from_stable_id, to_stable_id, to_name, kind, label) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![repo_id, edge.from.as_str(), to_stable_id, to_name, relationship_to_str(edge.kind), edge.label],
            )
            .map_err(sql_error)?;
        }

        tx.commit().map_err(sql_error)
    }

    pub fn load_repo_index(
        &self,
        root: &Path,
        fingerprint: &str,
    ) -> Result<Option<RepoStructureIndex>> {
        let root = root.to_string_lossy().to_string();
        let repo: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, fingerprint FROM repos WHERE root = ?1 AND schema_version = ?2",
                params![root, SCHEMA_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((repo_id, stored_fingerprint)) = repo else {
            return Ok(None);
        };
        if stored_fingerprint != fingerprint {
            return Ok(None);
        }

        let mut stmt = self.conn.prepare(
            "SELECT s.stable_id, s.name, s.qualified_name, s.kind, s.line_start, s.line_end, s.column_start, s.column_end, s.searchable_text, s.is_test, f.path, f.language
             FROM symbols s JOIN files f ON f.id = s.file_id WHERE s.repo_id = ?1 ORDER BY f.path, s.line_start, s.name",
        ).map_err(sql_error)?;
        let nodes = stmt
            .query_map(params![repo_id], |row| {
                let stable_id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let qualified_name: String = row.get(2)?;
                let kind: String = row.get(3)?;
                let line_start: u32 = row.get::<_, i64>(4)? as u32;
                let line_end: Option<u32> = row.get::<_, Option<i64>>(5)?.map(|v| v as u32);
                let column_start: Option<u32> = row.get::<_, Option<i64>>(6)?.map(|v| v as u32);
                let column_end: Option<u32> = row.get::<_, Option<i64>>(7)?.map(|v| v as u32);
                let searchable_text: String = row.get(8)?;
                let is_test: i64 = row.get(9)?;
                let path: String = row.get(10)?;
                let language: Option<String> = row.get(11)?;
                Ok(RepoNode {
                    id: RepoNodeId::new(stable_id),
                    symbol: SymbolIdentity {
                        name,
                        kind: str_to_symbol_kind(&kind),
                        container: None,
                        language,
                    },
                    qualified_name,
                    location: Location {
                        path,
                        range: Some(TextRange {
                            start: Position {
                                line: line_start,
                                column: column_start.unwrap_or(0),
                            },
                            end: Position {
                                line: line_end.unwrap_or(line_start),
                                column: column_end.unwrap_or(0),
                            },
                        }),
                    },
                    searchable_text,
                    is_test: is_test != 0,
                })
            })
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;

        let mut stmt = self.conn.prepare(
            "SELECT from_stable_id, to_stable_id, to_name, kind, label FROM edges WHERE repo_id = ?1 ORDER BY id",
        ).map_err(sql_error)?;
        let edges = stmt
            .query_map(params![repo_id], |row| {
                let from: String = row.get(0)?;
                let to_stable_id: Option<String> = row.get(1)?;
                let to_name: String = row.get(2)?;
                let kind: String = row.get(3)?;
                let label: Option<String> = row.get(4)?;
                Ok(RepoEdge {
                    from: RepoNodeId::new(from),
                    target: to_stable_id
                        .map(RepoNodeId::new)
                        .map(RepoEdgeTarget::Node)
                        .unwrap_or(RepoEdgeTarget::Symbol(to_name)),
                    kind: str_to_relationship(&kind),
                    label,
                })
            })
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;

        Ok(Some(RepoStructureIndex {
            workspace_root: Some(root),
            freshness: Freshness::Fresh,
            nodes,
            edges,
        }))
    }
}

fn ensure_file(
    tx: &rusqlite::Transaction<'_>,
    repo_id: i64,
    path: &str,
    language: Option<&str>,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO files(repo_id, path, language, content_hash, size, indexed_at, symbol_count, edge_count) VALUES(?1, ?2, ?3, '', 0, strftime('%s','now'), 0, 0)
         ON CONFLICT(repo_id, path) DO NOTHING",
        params![repo_id, path, language],
    ).map_err(sql_error)?;
    tx.query_row(
        "SELECT id FROM files WHERE repo_id = ?1 AND path = ?2",
        params![repo_id, path],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn range_parts(location: &Location) -> (u32, Option<u32>, Option<u32>, Option<u32>) {
    let Some(range) = &location.range else {
        return (1, None, None, None);
    };
    (
        range.start.line,
        Some(range.end.line),
        Some(range.start.column),
        Some(range.end.column),
    )
}

fn sql_error(error: rusqlite::Error) -> Error {
    Error::Tool(format!("code index sqlite error: {error}"))
}

fn symbol_kind_to_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::File => "file",
        SymbolKind::Module => "module",
        SymbolKind::Namespace => "namespace",
        SymbolKind::Package => "package",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Interface => "interface",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Constructor => "constructor",
        SymbolKind::Field => "field",
        SymbolKind::Property => "property",
        SymbolKind::Variable => "variable",
        SymbolKind::Constant => "constant",
        SymbolKind::TypeAlias => "type_alias",
        SymbolKind::Macro => "macro",
        SymbolKind::Unknown => "unknown",
    }
}

fn str_to_symbol_kind(kind: &str) -> SymbolKind {
    match kind {
        "file" => SymbolKind::File,
        "module" => SymbolKind::Module,
        "namespace" => SymbolKind::Namespace,
        "package" => SymbolKind::Package,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "interface" => SymbolKind::Interface,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "constructor" => SymbolKind::Constructor,
        "field" => SymbolKind::Field,
        "property" => SymbolKind::Property,
        "variable" => SymbolKind::Variable,
        "constant" => SymbolKind::Constant,
        "type_alias" => SymbolKind::TypeAlias,
        "macro" => SymbolKind::Macro,
        _ => SymbolKind::Unknown,
    }
}

fn relationship_to_str(kind: RepoRelationshipKind) -> &'static str {
    match kind {
        RepoRelationshipKind::Contains => "contains",
        RepoRelationshipKind::Implements => "implements",
        RepoRelationshipKind::Imports => "imports",
        RepoRelationshipKind::Calls => "calls",
        RepoRelationshipKind::Tests => "tests",
        RepoRelationshipKind::SameFile => "same_file",
    }
}

fn str_to_relationship(kind: &str) -> RepoRelationshipKind {
    match kind {
        "contains" => RepoRelationshipKind::Contains,
        "implements" => RepoRelationshipKind::Implements,
        "imports" => RepoRelationshipKind::Imports,
        "calls" => RepoRelationshipKind::Calls,
        "tests" => RepoRelationshipKind::Tests,
        _ => RepoRelationshipKind::SameFile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> RepoStructureIndex {
        RepoStructureIndex {
            workspace_root: Some("/tmp/project".to_string()),
            freshness: Freshness::Fresh,
            nodes: vec![
                RepoNode {
                    id: RepoNodeId::new("src/lib.rs:User:type"),
                    symbol: SymbolIdentity {
                        name: "User".to_string(),
                        kind: SymbolKind::Struct,
                        container: None,
                        language: Some("rust".to_string()),
                    },
                    qualified_name: "User".to_string(),
                    location: Location {
                        path: "src/lib.rs".to_string(),
                        range: Some(TextRange {
                            start: Position { line: 3, column: 0 },
                            end: Position { line: 5, column: 1 },
                        }),
                    },
                    searchable_text: "User id name".to_string(),
                    is_test: false,
                },
                RepoNode {
                    id: RepoNodeId::new("src/lib.rs:load_user:function"),
                    symbol: SymbolIdentity {
                        name: "load_user".to_string(),
                        kind: SymbolKind::Function,
                        container: None,
                        language: Some("rust".to_string()),
                    },
                    qualified_name: "load_user".to_string(),
                    location: Location {
                        path: "src/lib.rs".to_string(),
                        range: Some(TextRange {
                            start: Position { line: 7, column: 0 },
                            end: Position { line: 9, column: 1 },
                        }),
                    },
                    searchable_text: "fn load_user() -> User".to_string(),
                    is_test: false,
                },
            ],
            edges: vec![RepoEdge {
                from: RepoNodeId::new("src/lib.rs:load_user:function"),
                target: RepoEdgeTarget::Node(RepoNodeId::new("src/lib.rs:User:type")),
                kind: RepoRelationshipKind::Calls,
                label: Some("returns".to_string()),
            }],
        }
    }

    #[test]
    fn code_index_round_trips_repo_structure_index() {
        let mut store = CodeIndexStore::open_in_memory().unwrap();
        let root = Path::new("/tmp/project");
        let index = sample_index();

        store
            .write_repo_index(root, "fingerprint-1", &index)
            .unwrap();
        let loaded = store
            .load_repo_index(root, "fingerprint-1")
            .unwrap()
            .unwrap();

        assert_eq!(loaded.nodes.len(), 2);
        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(
            loaded.search("load_user", 5)[0].node.symbol.name,
            "load_user"
        );
        assert_eq!(loaded.related("src/lib.rs#load_user", 5).len(), 1);
    }

    #[test]
    fn scan_durable_index_loads_searchable_repo_structure() {
        let mut store = CodeIndexStore::open_in_memory().unwrap();
        let root = Path::new("/tmp/project");
        store
            .write_repo_index(root, "fingerprint-1", &sample_index())
            .unwrap();

        let loaded = store
            .load_repo_index(root, "fingerprint-1")
            .unwrap()
            .unwrap();
        let hits = loaded.search("User", 5);

        assert_eq!(hits[0].node.symbol.name, "User");
        assert_eq!(hits[0].node.location.path, "src/lib.rs");
    }

    #[test]
    fn code_index_rejects_stale_fingerprint() {
        let mut store = CodeIndexStore::open_in_memory().unwrap();
        let root = Path::new("/tmp/project");
        store
            .write_repo_index(root, "fresh", &sample_index())
            .unwrap();

        assert!(store.load_repo_index(root, "stale").unwrap().is_none());
    }
}
