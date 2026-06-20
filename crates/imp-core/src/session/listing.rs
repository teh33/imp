use std::path::{Path, PathBuf};

use imp_llm::{truncate_chars_with_suffix, Message};

use crate::error::{Error, Result};

use super::summary::{cleanup_summary_text, extract_text};
use super::{SessionEntry, SessionInfo};

pub(super) fn recent_session_files(session_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    for dir_entry in std::fs::read_dir(session_dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }

        let modified = dir_entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        let updated_at = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        files.push((path, updated_at, modified));
    }

    files.sort_by(|(path_a, _, modified_a), (path_b, _, modified_b)| {
        modified_b.cmp(modified_a).then_with(|| path_b.cmp(path_a))
    });
    Ok(files
        .into_iter()
        .map(|(path, updated_at, _)| (path, updated_at))
        .collect())
}

pub(super) fn session_info_matches(info: &SessionInfo, needle: &str) -> bool {
    info.name
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info
            .summary
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info
            .first_message
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
        || info.cwd.to_ascii_lowercase().contains(needle)
        || info.id.to_ascii_lowercase().contains(needle)
}

pub(super) fn read_session_info(path: &Path, updated_at: u64) -> Result<SessionInfo> {
    use std::io::BufRead;

    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut cwd = String::new();
    let mut created_at = 0;
    let mut message_count = 0;
    let mut compaction_count = 0;
    let mut first_message = None;
    let mut last_message = None;
    let mut name = None;
    let mut summary = None;
    let mut summary_parts = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<SessionEntry>(&line) else {
            continue;
        };
        match entry {
            SessionEntry::Header {
                cwd: entry_cwd,
                created_at: entry_created_at,
                ..
            } => {
                cwd = entry_cwd;
                created_at = entry_created_at;
            }
            SessionEntry::Message { message, .. } => {
                message_count += 1;
                if first_message.is_none() {
                    first_message = extract_text(&message);
                }
                if let Some(text) = extract_text(&message) {
                    last_message = Some(text);
                }
                if summary.is_none() && summary_parts.len() < 3 {
                    if let Message::Assistant(_) = message {
                        if let Some(text) = extract_text(&message) {
                            let trimmed = cleanup_summary_text(&text);
                            if !trimmed.is_empty() {
                                summary_parts.push(trimmed);
                            }
                        }
                    }
                }
            }
            SessionEntry::Compaction {
                summary: compaction_summary,
                ..
            } => {
                compaction_count += 1;
                if summary.is_none() {
                    let trimmed = cleanup_summary_text(&compaction_summary);
                    if !trimmed.is_empty() {
                        summary_parts.clear();
                        summary_parts.push(trimmed);
                    }
                }
            }
            SessionEntry::SessionMeta {
                name: meta_name,
                summary: meta_summary,
                ..
            } => {
                name = meta_name;
                if meta_summary
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    summary = meta_summary
                        .map(|value| truncate_chars_with_suffix(value.trim(), 120, "…"));
                }
            }
            _ => {}
        }
    }

    let summary = summary.or_else(|| {
        if summary_parts.is_empty() {
            None
        } else {
            let collapsed = summary_parts
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if collapsed.is_empty() {
                None
            } else {
                Some(truncate_chars_with_suffix(&collapsed, 120, "…"))
            }
        }
    });

    Ok(SessionInfo {
        id: path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.to_path_buf(),
        cwd,
        created_at,
        updated_at,
        message_count: if message_count == 0 {
            compaction_count
        } else {
            message_count
        },
        first_message,
        last_message,
        name,
        summary,
    })
}

/// Read just the first non-empty line of a file.
pub(super) fn read_first_line(path: &Path) -> Result<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            return Ok(line);
        }
    }
    Err(Error::Session("empty file".into()))
}
