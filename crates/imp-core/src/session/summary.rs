use imp_llm::{truncate_chars_with_suffix, Message};

use super::SessionEntry;

/// Extract the first text content from a message.
pub(super) fn extract_text(message: &Message) -> Option<String> {
    let blocks = match message {
        Message::User(u) => &u.content,
        Message::Assistant(a) => &a.content,
        Message::ToolResult(t) => &t.content,
    };
    blocks.iter().find_map(|b| match b {
        imp_llm::ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    })
}

pub(super) fn derive_session_summary(entries: &[SessionEntry]) -> Option<String> {
    let mut parts = Vec::new();

    for entry in entries.iter().rev() {
        match entry {
            SessionEntry::SessionMeta {
                summary: Some(summary),
                ..
            } if !summary.trim().is_empty() => {
                return Some(truncate_chars_with_suffix(summary.trim(), 120, "…"));
            }
            // Session summaries are stored in compact session-meta entries.
            SessionEntry::Compaction { summary, .. } => {
                let trimmed = cleanup_summary_text(summary);
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            SessionEntry::CompactionV2 { record, .. } => {
                let trimmed = cleanup_summary_text(&record.summary);
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            SessionEntry::Message { message, .. } => {
                if let Message::Assistant(_) = message {
                    if let Some(text) = extract_text(message) {
                        let trimmed = cleanup_summary_text(&text);
                        if !trimmed.is_empty() {
                            parts.push(trimmed);
                        }
                    }
                }
            }
            _ => {}
        }

        if parts.len() >= 3 {
            break;
        }
    }

    if parts.is_empty() {
        return None;
    }

    let joined = parts.into_iter().rev().collect::<Vec<_>>().join(" ");
    let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(truncate_chars_with_suffix(&collapsed, 120, "…"))
    }
}

pub(super) fn cleanup_summary_text(text: &str) -> String {
    let mut collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    for prefix in [
        "summary:",
        "session summary:",
        "assistant summary:",
        "in summary,",
        "to summarize,",
    ] {
        if collapsed.to_ascii_lowercase().starts_with(prefix) {
            collapsed = collapsed[prefix.len()..].trim().to_string();
            break;
        }
    }

    collapsed
}

pub(super) fn preferred_title_candidate(
    first_prompt: Option<&str>,
    summary: Option<&str>,
    max_chars: usize,
) -> Option<String> {
    let first_prompt = first_prompt
        .map(cleanup_summary_text)
        .filter(|text| !text.is_empty());
    let summary = summary
        .map(cleanup_summary_text)
        .filter(|text| !text.is_empty());

    match (first_prompt.as_deref(), summary.as_deref()) {
        (Some(prompt), Some(summary)) => {
            let prompt_title = literal_topic_title(prompt, max_chars);
            let summary_title = literal_topic_title(summary, max_chars);
            choose_better_title(prompt_title, summary_title, max_chars)
        }
        (Some(prompt), None) => literal_topic_title(prompt, max_chars),
        (None, Some(summary)) => literal_topic_title(summary, max_chars),
        (None, None) => None,
    }
}

fn choose_better_title(
    prompt_title: Option<String>,
    summary_title: Option<String>,
    max_chars: usize,
) -> Option<String> {
    match (prompt_title, summary_title) {
        (Some(prompt), Some(summary)) => {
            if is_generic_title(&prompt) && !is_generic_title(&summary) {
                Some(summary)
            } else if !is_generic_title(&prompt) && is_generic_title(&summary) {
                Some(prompt)
            } else if topic_word_count(&summary) > topic_word_count(&prompt) {
                Some(summary)
            } else {
                Some(truncate_chars_with_suffix(&prompt, max_chars, "…"))
            }
        }
        (Some(prompt), None) => Some(prompt),
        (None, Some(summary)) => Some(summary),
        (None, None) => None,
    }
}

fn topic_word_count(title: &str) -> usize {
    title
        .split_whitespace()
        .filter(|word| word.len() >= 4)
        .count()
}

pub(super) fn literal_topic_title(text: &str, max_chars: usize) -> Option<String> {
    let cleaned = cleanup_summary_text(text);
    if cleaned.is_empty() {
        return None;
    }

    let literal = concise_topic_phrase(&cleaned, max_chars);
    if !literal.trim().is_empty() && !is_generic_title(&literal) {
        return Some(literal);
    }

    let heuristic = summarize_session_title(&cleaned, max_chars);
    if !heuristic.trim().is_empty() && !is_generic_title(&heuristic) {
        return Some(heuristic);
    }

    Some(truncate_chars_with_suffix(cleaned.trim(), max_chars, "…"))
}

fn is_generic_title(title: &str) -> bool {
    let lower = title.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }

    let generic_words = [
        "yes", "yeah", "yep", "ok", "okay", "sure", "think", "some", "pretty", "good", "great",
        "nice", "maybe", "just", "really", "thing", "stuff",
    ];

    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() <= 2 && words.iter().all(|w| generic_words.contains(w)) {
        return true;
    }

    words.iter().filter(|w| generic_words.contains(w)).count() >= words.len().saturating_sub(1)
}

fn concise_topic_phrase(text: &str, max_chars: usize) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    let mut phrase = collapsed
        .split_terminator(['.', '!', '?', ';', ':'])
        .find_map(|part| {
            let trimmed = part.trim();
            if trimmed.split_whitespace().count() >= 3 {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .unwrap_or(collapsed);

    let leading_phrases = [
        "we should ",
        "let's ",
        "i want to ",
        "i'd like to ",
        "can we ",
        "can you ",
        "could we ",
        "could you ",
        "would you ",
        "please ",
        "help me ",
        "yes ",
        "yeah ",
        "ok ",
        "okay ",
        "sure ",
        "i think ",
        "think ",
    ];

    let lower = phrase.to_ascii_lowercase();
    for prefix in leading_phrases {
        if let Some(stripped) = lower.strip_prefix(prefix) {
            phrase = stripped.trim().to_string();
            break;
        }
    }

    let stopwords = [
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "but",
        "by",
        "for",
        "from",
        "how",
        "i",
        "if",
        "in",
        "into",
        "is",
        "it",
        "its",
        "me",
        "my",
        "of",
        "on",
        "or",
        "please",
        "so",
        "that",
        "the",
        "their",
        "them",
        "there",
        "these",
        "they",
        "this",
        "to",
        "up",
        "we",
        "what",
        "when",
        "where",
        "which",
        "while",
        "with",
        "would",
        "can",
        "could",
        "should",
        "work",
        "working",
        "improving",
        "improve",
        "usability",
        "currently",
        "displayed",
        "shown",
        "information",
        "some",
        "pretty",
        "really",
        "just",
        "think",
        "yes",
        "yeah",
        "okay",
        "ok",
        "sure",
    ];

    let normalized = phrase
        .replace("/resume", "resume")
        .replace("chat summaries", "chat_summaries")
        .replace("top bar", "top_bar")
        .replace("session picker", "session_picker")
        .replace("oauth login", "oauth_login")
        .replace("provider refresh", "provider_refresh");

    let mut tokens = Vec::new();
    for raw in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if stopwords.contains(&lower.as_str()) {
            continue;
        }
        if tokens.iter().any(|existing: &String| existing == &lower) {
            continue;
        }
        tokens.push(lower);
    }

    if tokens.is_empty() {
        let words: Vec<&str> = phrase.split_whitespace().collect();
        let take = words.len().min(4);
        return truncate_chars_with_suffix(&words[..take].join(" "), max_chars, "…");
    }

    let mut out = tokens
        .into_iter()
        .take(5)
        .map(|token| match token.as_str() {
            "chat_summaries" => "chat summaries".to_string(),
            "top_bar" => "top bar".to_string(),
            "session_picker" => "session picker".to_string(),
            "oauth_login" => "oauth login".to_string(),
            "provider_refresh" => "provider refresh".to_string(),
            _ => token,
        })
        .collect::<Vec<_>>();

    if out.len() > 4 {
        out.truncate(4);
    }

    let mut out = out.join(" ");

    out = out.replace("resume chat summaries", "resume + summaries");
    truncate_chars_with_suffix(out.trim(), max_chars, "…")
}

pub(super) fn summarize_session_title(text: &str, max_chars: usize) -> String {
    let collapsed = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut normalized = collapsed.to_ascii_lowercase();

    for prefix in [
        "can we ",
        "could we ",
        "can you ",
        "could you ",
        "would you ",
        "please ",
        "please can you ",
        "please could you ",
        "help me ",
        "i want to ",
        "i'd like to ",
        "let's ",
    ] {
        if let Some(stripped) = normalized.strip_prefix(prefix) {
            normalized = stripped.to_string();
            break;
        }
    }

    for (phrase, token) in [
        ("top bar", "top_bar"),
        ("prompt box", "prompt_box"),
        ("thinking level", "thinking_level"),
        ("model name", "model_name"),
        ("session name", "session_name"),
        ("chat title", "chat_title"),
        ("chat name", "chat_name"),
        ("session id", "session_id"),
        ("context window", "context_window"),
    ] {
        normalized = normalized.replace(phrase, token);
    }

    let mentions_top_bar_layout = normalized.contains("top_bar")
        && (normalized.contains("display")
            || normalized.contains("displayed")
            || normalized.contains("shown")
            || normalized.contains("information"));

    let verbs = [
        "fix",
        "adjust",
        "update",
        "change",
        "move",
        "rename",
        "remove",
        "add",
        "show",
        "hide",
        "improve",
        "refactor",
        "debug",
        "investigate",
        "implement",
        "summarize",
    ];

    let stopwords = [
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "but",
        "by",
        "for",
        "from",
        "get",
        "have",
        "how",
        "i",
        "if",
        "in",
        "instead",
        "into",
        "is",
        "it",
        "its",
        "me",
        "my",
        "now",
        "of",
        "on",
        "or",
        "please",
        "right",
        "so",
        "string",
        "that",
        "the",
        "their",
        "them",
        "then",
        "there",
        "these",
        "they",
        "this",
        "to",
        "up",
        "we",
        "what",
        "when",
        "where",
        "which",
        "while",
        "with",
        "would",
        "listed",
        "resume",
        "prompt",
        "first",
        "summarized",
        "summarize",
        "information",
        "display",
        "displayed",
        "shown",
        "currently",
    ];

    let mut verb: Option<String> = None;
    let mut nouns: Vec<String> = Vec::new();

    for raw in normalized.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        if verb.is_none() && verbs.contains(&raw) {
            verb = Some(raw.to_string());
            continue;
        }
        if stopwords.contains(&raw) {
            continue;
        }
        if nouns.iter().any(|existing| existing == raw) {
            continue;
        }
        nouns.push(raw.to_string());
    }

    let mut parts = Vec::new();
    if let Some(verb) = verb {
        parts.push(verb);
    }

    for noun in nouns {
        if parts.len() >= 4 {
            break;
        }
        parts.push(noun.clone());
        if noun == "top_bar" && mentions_top_bar_layout && parts.len() < 4 {
            parts.push("layout".to_string());
        }
    }

    if parts.is_empty() {
        parts.push(collapsed.trim().to_string());
    }

    let summary = parts
        .into_iter()
        .map(|part| match part.as_str() {
            "top_bar" => "top bar".to_string(),
            "prompt_box" => "prompt box".to_string(),
            "thinking_level" => "thinking level".to_string(),
            "model_name" => "model name".to_string(),
            "session_name" => "session name".to_string(),
            "chat_title" => "chat title".to_string(),
            "chat_name" => "chat name".to_string(),
            "session_id" => "session id".to_string(),
            "context_window" => "context window".to_string(),
            _ => part,
        })
        .collect::<Vec<_>>()
        .join(" ");

    truncate_chars_with_suffix(summary.trim(), max_chars, "…")
}
