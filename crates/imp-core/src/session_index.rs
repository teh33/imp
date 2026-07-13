use std::path::Path;

use imp_llm::truncate_chars_with_suffix;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;
use crate::session::{SessionEntry, SessionManager};

/// SQLite FTS5 index over past sessions for cross-session search.
pub struct SessionIndex {
    db: Connection,
}

/// A search hit from the session index.
#[derive(Debug, Clone)]
pub struct SessionSearchHit {
    pub session_id: String,
    pub cwd: String,
    pub created_at: u64,
    pub snippet: String,
    pub message_count: usize,
    pub first_message: Option<String>,
}

impl SessionIndex {
    /// Open or create the session index database.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let db = Connection::open(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                message_count INTEGER NOT NULL,
                first_message TEXT
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS session_content USING fts5(
                session_id,
                content,
                tokenize='porter unicode61'
            );",
        )?;
        Ok(Self { db })
    }

    /// Index a session's content. Extracts user messages and assistant text
    /// (not tool results — too noisy), plus compaction summaries.
    ///
    /// Idempotent: re-indexing the same session updates the existing entry.
    pub fn index_session(&self, session: &SessionManager) -> Result<()> {
        let session_id = session
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let mut cwd = String::new();
        let mut created_at: u64 = 0;
        let mut message_count: usize = 0;
        let mut first_message: Option<String> = None;
        let mut content_parts: Vec<String> = Vec::new();

        for entry in session.entries() {
            match entry {
                SessionEntry::Header {
                    cwd: c,
                    created_at: t,
                    ..
                } => {
                    cwd = c.clone();
                    created_at = *t;
                }
                SessionEntry::Message { message, .. } => {
                    message_count += 1;
                    let text = extract_message_text(message);
                    if !text.is_empty() {
                        if first_message.is_none() {
                            first_message = Some(truncate(&text, 200));
                        }
                        content_parts.push(text);
                    }
                }
                SessionEntry::Compaction { summary, .. } => {
                    content_parts.push(summary.clone());
                }
                SessionEntry::CompactionV2 { record, .. } => {
                    content_parts.push(record.summary.clone());
                }
                _ => {}
            }
        }

        if content_parts.is_empty() {
            return Ok(());
        }

        let content = content_parts.join("\n");

        // Upsert session metadata
        self.db.execute(
            "INSERT INTO sessions (id, cwd, created_at, message_count, first_message)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                message_count = excluded.message_count,
                first_message = excluded.first_message",
            params![
                session_id,
                cwd,
                created_at as i64,
                message_count as i64,
                first_message
            ],
        )?;

        // Delete old FTS content and re-insert
        self.db.execute(
            "DELETE FROM session_content WHERE session_id = ?1",
            params![session_id],
        )?;
        self.db.execute(
            "INSERT INTO session_content (session_id, content) VALUES (?1, ?2)",
            params![session_id, content],
        )?;

        Ok(())
    }

    /// Full-text search across indexed sessions.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SessionSearchHit>> {
        let mut stmt = self.db.prepare(
            "SELECT
                sc.session_id,
                s.cwd,
                s.created_at,
                snippet(session_content, 1, '>>>', '<<<', '...', 40) as snippet,
                s.message_count,
                s.first_message
             FROM session_content sc
             JOIN sessions s ON s.id = sc.session_id
             WHERE session_content MATCH ?1
             ORDER BY rank, s.created_at DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SessionSearchHit {
                session_id: row.get(0)?,
                cwd: row.get(1)?,
                created_at: row.get::<_, i64>(2)? as u64,
                snippet: row.get(3)?,
                message_count: row.get::<_, i64>(4)? as usize,
                first_message: row.get(5)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Check if a session is already indexed.
    pub fn is_indexed(&self, session_id: &str) -> bool {
        self.db
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                params![session_id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
}

/// Extract searchable text from a message. Skips tool results (too noisy).
fn extract_message_text(message: &imp_llm::Message) -> String {
    let blocks = match message {
        imp_llm::Message::User(u) => &u.content,
        imp_llm::Message::Assistant(a) => &a.content,
        imp_llm::Message::ToolResult(_) => return String::new(),
    };

    blocks
        .iter()
        .filter_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    truncate_chars_with_suffix(s, max, "...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionManager;
    use tempfile::TempDir;

    fn make_session_with_messages(dir: &std::path::Path, texts: &[&str]) -> SessionManager {
        let session_dir = dir.join("sessions");
        let cwd = dir.join("project");
        let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();

        for (i, text) in texts.iter().enumerate() {
            let entry = SessionEntry::Message {
                id: format!("m{i}"),
                parent_id: None,
                message: imp_llm::Message::user(*text),
            };
            mgr.append(entry).unwrap();

            // Add an assistant response
            let reply = SessionEntry::Message {
                id: format!("a{i}"),
                parent_id: None,
                message: imp_llm::Message::Assistant(imp_llm::AssistantMessage {
                    content: vec![imp_llm::ContentBlock::Text {
                        text: format!("Response to: {text}"),
                    }],
                    usage: None,
                    stop_reason: imp_llm::StopReason::EndTurn,
                    timestamp: 0,
                }),
            };
            mgr.append(reply).unwrap();
        }

        mgr
    }

    #[test]
    fn session_index_create_and_search() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let session = make_session_with_messages(
            dir.path(),
            &["Help me deploy to kubernetes", "Show me the docker config"],
        );
        index.index_session(&session).unwrap();

        let results = index.search("kubernetes", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("kubernetes"));
    }

    #[test]
    fn session_index_no_results() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let session = make_session_with_messages(dir.path(), &["Hello world"]);
        index.index_session(&session).unwrap();

        let results = index.search("kubernetes", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn session_index_multiple_sessions() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let s1 = make_session_with_messages(dir.path(), &["Deploy to kubernetes cluster"]);
        index.index_session(&s1).unwrap();

        // Create second session in a different subdir to get a different session file
        let dir2 = dir.path().join("other");
        std::fs::create_dir_all(&dir2).unwrap();
        let s2 = make_session_with_messages(&dir2, &["Fix the kubernetes ingress"]);
        index.index_session(&s2).unwrap();

        let results = index.search("kubernetes", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn session_index_idempotent() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let session = make_session_with_messages(dir.path(), &["test content"]);
        index.index_session(&session).unwrap();
        index.index_session(&session).unwrap(); // Re-index

        let results = index.search("test", 10).unwrap();
        assert_eq!(results.len(), 1, "should not duplicate on re-index");
    }

    #[test]
    fn session_index_is_indexed() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        assert!(!index.is_indexed("nonexistent"));

        let session = make_session_with_messages(dir.path(), &["hello"]);
        index.index_session(&session).unwrap();

        let session_id = session
            .path()
            .unwrap()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(index.is_indexed(&session_id));
    }

    #[test]
    fn session_index_fts5_and_or_not() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let session = make_session_with_messages(
            dir.path(),
            &["Deploy kubernetes cluster", "Configure docker networking"],
        );
        index.index_session(&session).unwrap();

        // AND query
        let results = index.search("kubernetes AND cluster", 10).unwrap();
        assert_eq!(results.len(), 1);

        // OR query
        let results = index.search("kubernetes OR docker", 10).unwrap();
        assert_eq!(results.len(), 1); // same session has both

        // NOT query
        let results = index.search("kubernetes NOT docker", 10).unwrap();
        // FTS5 NOT: matches docs containing kubernetes but not docker
        // Since both are in the same session content, this should return 0
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn session_index_snippet_highlights() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = SessionIndex::open(&db_path).unwrap();

        let session =
            make_session_with_messages(dir.path(), &["The kubernetes deployment is broken"]);
        index.index_session(&session).unwrap();

        let results = index.search("kubernetes", 10).unwrap();
        assert_eq!(results.len(), 1);
        // Snippet should contain >>> and <<< markers
        assert!(
            results[0].snippet.contains(">>>") && results[0].snippet.contains("<<<"),
            "snippet should have highlight markers: {}",
            results[0].snippet
        );
    }
}
