use super::*;

// ── levenshtein ───────────────────────────────────────────────────

#[test]
fn suggest_similar_levenshtein_identical() {
    assert_eq!(levenshtein("hello", "hello"), 0);
}

#[test]
fn suggest_similar_levenshtein_one_substitution() {
    assert_eq!(levenshtein("auth", "aath"), 1);
}

#[test]
fn suggest_similar_levenshtein_one_insertion() {
    assert_eq!(levenshtein("helo", "hello"), 1);
}

#[test]
fn suggest_similar_levenshtein_one_deletion() {
    assert_eq!(levenshtein("hello", "helo"), 1);
}

#[test]
fn suggest_similar_levenshtein_empty_strings() {
    assert_eq!(levenshtein("", ""), 0);
    assert_eq!(levenshtein("abc", ""), 3);
    assert_eq!(levenshtein("", "abc"), 3);
}

#[test]
fn suggest_similar_levenshtein_completely_different() {
    // "abc" vs "xyz": 3 substitutions
    assert_eq!(levenshtein("abc", "xyz"), 3);
}

#[test]
fn suggest_similar_levenshtein_transposition() {
    // "atuh" vs "auth": swap two adjacent chars = distance 2
    assert_eq!(levenshtein("atuh", "auth"), 2);
}

// ── suggest_similar_files ─────────────────────────────────────────

#[test]
fn suggest_similar_finds_close_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("middleware.rs"), "").unwrap();
    std::fs::write(dir.path().join("unrelated.rs"), "").unwrap();

    let suggestions = suggest_similar_files(dir.path(), "middlewar.rs");
    assert!(
        suggestions.iter().any(|s| s.contains("middleware.rs")),
        "expected middleware.rs in suggestions, got: {suggestions:?}"
    );
}

#[test]
fn suggest_similar_returns_at_most_three() {
    let dir = tempfile::tempdir().unwrap();
    // Create five files each 1 edit away from "xod.rs"
    for name in &["mod.rs", "rod.rs", "cod.rs", "nod.rs", "pod.rs"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }

    let suggestions = suggest_similar_files(dir.path(), "xod.rs");
    assert!(suggestions.len() <= 3);
}

#[test]
fn suggest_similar_nothing_close_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("completely_different.rs"), "").unwrap();

    // "a.rs" is far from "completely_different.rs"
    let suggestions = suggest_similar_files(dir.path(), "a.rs");
    assert!(
        suggestions.is_empty(),
        "expected no suggestions, got: {suggestions:?}"
    );
}

#[test]
fn suggest_similar_ranks_closer_matches_first() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("auth.rs"), "").unwrap();
    std::fs::write(dir.path().join("autho.rs"), "").unwrap();

    let suggestions = suggest_similar_files(dir.path(), "atuh.rs");
    assert!(!suggestions.is_empty());
    assert!(
        suggestions.iter().any(|s| s.contains("auth.rs")),
        "expected auth.rs, got: {suggestions:?}"
    );
}

fn simple_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "count": { "type": "integer" }
        },
        "required": ["path"]
    })
}

#[test]
fn validate_tool_args_valid_passes() {
    let schema = simple_schema();
    let args = serde_json::json!({ "path": "/tmp/foo.txt" });
    assert!(validate_tool_args(&schema, &args).is_ok());
}

#[test]
fn validate_tool_args_valid_with_optional_passes() {
    let schema = simple_schema();
    let args = serde_json::json!({ "path": "/tmp/foo.txt", "count": 5 });
    assert!(validate_tool_args(&schema, &args).is_ok());
}

#[test]
fn validate_tool_args_missing_required_returns_error() {
    let schema = simple_schema();
    // Missing the required "path" field
    let args = serde_json::json!({ "count": 5 });
    let result = validate_tool_args(&schema, &args);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("path") || msg.contains("required"),
        "expected error mentioning 'path' or 'required', got: {msg}"
    );
}

#[test]
fn validate_tool_args_wrong_type_returns_error() {
    let schema = simple_schema();
    // "count" must be integer, not string
    let args = serde_json::json!({ "path": "/tmp/foo.txt", "count": "not-a-number" });
    let result = validate_tool_args(&schema, &args);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("integer") || msg.contains("type"),
        "expected type error, got: {msg}"
    );
}

#[test]
fn validate_tool_args_extra_fields_allowed() {
    // LLMs often add extra fields — we should not reject them
    let schema = simple_schema();
    let args = serde_json::json!({
        "path": "/tmp/foo.txt",
        "llm_added_extra": "some value",
        "another_unknown": 42
    });
    assert!(
        validate_tool_args(&schema, &args).is_ok(),
        "extra/unknown fields should be allowed"
    );
}

// ── FileTracker ───────────────────────────────────────────────────

#[test]
fn file_track_was_read_false_for_unread_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let tracker = FileTracker::new();
    assert!(!tracker.was_read(&file), "unread file should return false");
}

#[test]
fn file_track_was_read_true_after_recording() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let mut tracker = FileTracker::new();
    tracker.record_read(&file);
    assert!(
        tracker.was_read(&file),
        "file should be marked as read after recording"
    );
}

#[test]
fn file_track_is_stale_false_for_unread_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let tracker = FileTracker::new();
    // Unread file is never stale (no baseline to compare against)
    assert!(!tracker.is_stale(&file));
}

#[test]
fn file_track_is_stale_false_immediately_after_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let mut tracker = FileTracker::new();
    tracker.record_read(&file);
    // No modification since read — should not be stale
    assert!(!tracker.is_stale(&file));
}

#[test]
fn file_track_is_stale_detects_external_modification() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original content").unwrap();

    let mut tracker = FileTracker::new();
    tracker.record_read(&file);

    // Set the file's mtime to 2 seconds in the future to guarantee a detectable change.
    // std::fs::File::set_modified is stable since Rust 1.75 and needs no extra crate.
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&file) {
        let _ = f.set_modified(future);
    }

    assert!(
        tracker.is_stale(&file),
        "file with advanced mtime should be stale"
    );
}

// ── FileHistory tests ─────────────────────────────────────

#[test]
fn file_history_snapshot_stores_original() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    let history = FileHistory::new();
    history.snapshot_before_edit(&file).unwrap();

    assert_eq!(history.original(&file).unwrap(), "fn main() {}");
}

#[test]
fn file_history_second_snapshot_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "original").unwrap();

    let history = FileHistory::new();
    history.snapshot_before_edit(&file).unwrap();

    // Modify the file and snapshot again — should keep original
    std::fs::write(&file, "modified").unwrap();
    history.snapshot_before_edit(&file).unwrap();

    assert_eq!(history.original(&file).unwrap(), "original");
}

#[test]
fn file_history_rollback_restores_original() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "original content").unwrap();

    let history = FileHistory::new();
    history.snapshot_before_edit(&file).unwrap();

    std::fs::write(&file, "agent wrote this").unwrap();
    history.rollback(&file).unwrap();

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original content");
}

#[test]
fn file_history_skips_nonexistent_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("does_not_exist.rs");

    let history = FileHistory::new();
    history.snapshot_before_edit(&file).unwrap();

    assert!(history.original(&file).is_none());
}

#[test]
fn file_history_tracked_files_lists_all() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("a.rs");
    let f2 = dir.path().join("b.rs");
    std::fs::write(&f1, "a").unwrap();
    std::fs::write(&f2, "b").unwrap();

    let history = FileHistory::new();
    history.snapshot_before_edit(&f1).unwrap();
    history.snapshot_before_edit(&f2).unwrap();

    let tracked = history.tracked_files();
    assert_eq!(tracked.len(), 2);
    assert!(tracked.contains(&f1));
    assert!(tracked.contains(&f2));
}
