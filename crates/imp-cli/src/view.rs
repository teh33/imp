use std::io;

use imp_core::session::{SessionEntry, SessionManager};
use imp_llm::{truncate_chars_with_suffix, Message};

pub(crate) async fn run(area: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let session_dir = imp_core::storage::global_sessions_dir();

    match area.unwrap_or("sessions") {
        "sessions" => show_sessions(&session_dir),
        "tree" => {
            let session = recent_session(&cwd, &session_dir)?;
            let tree = session.get_tree();
            if tree.is_empty() {
                println!("No session history yet.");
                return Ok(());
            }

            println!("Session tree\n============");
            print_tree_nodes(&tree, 0);
            Ok(())
        }
        "logs" => {
            let session = recent_session(&cwd, &session_dir)?;
            println!("Session log\n===========");
            for entry in session.entries().iter().rev().take(40).rev() {
                println!("{}", summarize_session_entry(entry));
            }
            Ok(())
        }
        "checkpoints" => {
            let session = recent_session(&cwd, &session_dir)?;
            let checkpoints = session.checkpoint_records();
            if checkpoints.is_empty() {
                println!("No checkpoints recorded in the most recent session for this working directory.");
                return Ok(());
            }

            println!("Checkpoints\n===========");
            for checkpoint in checkpoints {
                let label = checkpoint
                    .label
                    .as_deref()
                    .map(|label| format!(" — {label}"))
                    .unwrap_or_default();
                println!(
                    "- {}{} ({} file{})",
                    checkpoint.checkpoint_id,
                    label,
                    checkpoint.files.len(),
                    if checkpoint.files.len() == 1 { "" } else { "s" }
                );
                for file in checkpoint.files.iter().take(8) {
                    println!("    {file}");
                }
                if checkpoint.files.len() > 8 {
                    println!("    … {} more", checkpoint.files.len() - 8);
                }
            }
            Ok(())
        }
        other => {
            eprintln!(
                "Unknown viewer area: {other}. Use one of: sessions, tree, logs, checkpoints."
            );
            Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown viewer area").into())
        }
    }
}

fn show_sessions(session_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let sessions = SessionManager::list(session_dir)?;
    if sessions.is_empty() {
        println!("No saved sessions found.");
        return Ok(());
    }

    println!("Sessions\n========");
    for (idx, session) in sessions.iter().enumerate().take(20) {
        let title = session.title(72).unwrap_or_else(|| session.id.clone());
        let project = session.cwd.clone();
        println!("{}. {}", idx + 1, title);
        println!("   id: {}", session.id);
        println!("   project: {}", project);
        println!("   path: {}", session.path.display());
        println!("   messages: {}", session.message_count);
        if let Some(summary) = &session.summary {
            println!(
                "   summary: {}",
                truncate_chars_with_suffix(summary, 120, "…")
            );
        }
    }
    if sessions.len() > 20 {
        println!("… {} more session(s)", sessions.len() - 20);
    }
    Ok(())
}

fn recent_session(
    cwd: &std::path::Path,
    session_dir: &std::path::Path,
) -> Result<SessionManager, Box<dyn std::error::Error>> {
    Ok(
        SessionManager::continue_recent(cwd, session_dir)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "No recent session found for this working directory.",
            )
        })?,
    )
}

fn print_tree_nodes(nodes: &[imp_core::session::TreeNode], depth: usize) {
    for node in nodes {
        let indent = "  ".repeat(depth);
        let summary = match &node.entry {
            SessionEntry::Header { cwd, .. } => format!("header {cwd}"),
            SessionEntry::SessionMeta { name, summary, .. } => format!(
                "session-meta {}{}",
                name.as_deref().unwrap_or("(unnamed)"),
                summary
                    .as_deref()
                    .map(|s| format!(" — {}", truncate_chars_with_suffix(s, 60, "…")))
                    .unwrap_or_default()
            ),
            SessionEntry::Message { message, .. } => summarize_message_for_view(message),
            SessionEntry::Compaction { summary, .. } => {
                format!(
                    "compaction {}",
                    truncate_chars_with_suffix(summary, 60, "…")
                )
            }
            SessionEntry::Label { label, .. } => format!("label {label}"),
            SessionEntry::Custom { custom_type, .. } => format!("custom {custom_type}"),
        };
        println!("{indent}- {summary}");
        print_tree_nodes(&node.children, depth + 1);
    }
}

fn summarize_message_for_view(message: &Message) -> String {
    let text_content = |message: &Message| -> Option<String> {
        let blocks = match message {
            Message::User(user) => &user.content,
            Message::Assistant(assistant) => &assistant.content,
            Message::ToolResult(result) => &result.content,
        };
        blocks.iter().find_map(|block| match block {
            imp_llm::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
    };

    match message {
        Message::User(user) => format!(
            "user {}",
            truncate_chars_with_suffix(
                &text_content(message).unwrap_or_else(|| {
                    user.content
                        .iter()
                        .filter_map(|block| match block {
                            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
                80,
                "…"
            )
        ),
        Message::Assistant(_) => format!(
            "assistant {}",
            truncate_chars_with_suffix(&text_content(message).unwrap_or_default(), 80, "…")
        ),
        Message::ToolResult(result) => format!(
            "tool-result {}",
            truncate_chars_with_suffix(
                &text_content(message).unwrap_or_else(|| result.tool_call_id.clone()),
                80,
                "…"
            )
        ),
    }
}

fn summarize_session_entry(entry: &SessionEntry) -> String {
    match entry {
        SessionEntry::Header { cwd, .. } => format!("header cwd={cwd}"),
        SessionEntry::SessionMeta { name, summary, .. } => format!(
            "session-meta name={} summary={}",
            name.as_deref().unwrap_or("(unnamed)"),
            summary.as_deref().unwrap_or("(none)")
        ),
        SessionEntry::Message { message, .. } => summarize_message_for_view(message),
        SessionEntry::Compaction { summary, .. } => {
            format!(
                "compaction {}",
                truncate_chars_with_suffix(summary, 100, "…")
            )
        }
        SessionEntry::Label { label, .. } => format!("label {label}"),
        SessionEntry::Custom { custom_type, .. } => format!("custom {custom_type}"),
    }
}
