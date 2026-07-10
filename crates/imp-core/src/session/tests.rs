use super::*;
use async_trait::async_trait;
use futures::stream;
use imp_llm::{
    auth::{ApiKey, AuthStore},
    model::{Capabilities, ModelMeta, ModelPricing},
    provider::{Context, Provider, RequestOptions},
    AssistantMessage, ContentBlock, Message, StopReason, StreamEvent,
};
use tempfile::TempDir;

struct NoopProvider {
    models: Vec<ModelMeta>,
}

#[async_trait]
impl Provider for NoopProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> std::pin::Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>
    {
        Box::pin(stream::empty())
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok(String::new())
    }

    fn id(&self) -> &str {
        "noop"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

fn make_test_model() -> Model {
    Model {
        meta: ModelMeta {
            id: "test-model".into(),
            provider: "test-provider".into(),
            name: "Test Model".into(),
            context_window: 8192,
            max_output_tokens: 2048,
            pricing: ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 1.0,
            },
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
        provider: std::sync::Arc::new(NoopProvider { models: Vec::new() }),
    }
}

fn make_msg_entry(id: &str, text: &str) -> SessionEntry {
    SessionEntry::Message {
        id: id.to_string(),
        parent_id: None, // append() will set this
        message: Message::user(text),
    }
}

#[test]
fn summarized_title_compacts_request_into_short_label() {
    let title = summarize_session_title(
        "can we adjust the information that is displayed in the top bar",
        48,
    );
    assert_eq!(title, "adjust top bar layout");
}

#[test]
fn literal_topic_title_prefers_subject_words_over_compaction() {
    let title = literal_topic_title(
        "can we work on improving the usability of /resume and the chat summaries?",
        64,
    )
    .unwrap();

    assert!(title.contains("resume") || title.contains("summaries"));
    assert!(title.split_whitespace().count() <= 5);
}

#[test]
fn generic_summary_title_falls_back_to_more_descriptive_phrase() {
    let title = literal_topic_title(
            "yes think some pretty significant issues with oauth login persistence and provider refresh",
            64,
        )
        .unwrap();

    assert!(title.contains("oauth") || title.contains("login"));
    assert!(title.split_whitespace().count() <= 5);
    assert_ne!(title, "yes think some pretty");
}

#[test]
fn session_titles_can_be_derived_from_summary_text() {
    let info = SessionInfo {
        id: "abc".into(),
        path: PathBuf::from("/tmp/abc.jsonl"),
        cwd: "/tmp/project".into(),
        created_at: 0,
        updated_at: 0,
        message_count: 1,
        first_message: Some("help me with oauth login issues".into()),
        last_message: Some("fixed oauth login issues".into()),
        name: None,
        summary: Some("Investigated OAuth login failures and refreshed provider auth flow".into()),
    };

    let title = info.title(48).unwrap();
    assert!(!title.is_empty());
    assert!(title.contains("oauth") || title.contains("login") || title.contains("provider"));
    assert!(title.split_whitespace().count() <= 5);
}

#[test]
fn session_compaction_active_messages_replace_prefix_with_summary() {
    let mut mgr = SessionManager::in_memory();

    mgr.append(make_msg_entry("u1", "first request")).unwrap();
    mgr.append(SessionEntry::Message {
        id: "a1".into(),
        parent_id: None,
        message: Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "initial answer".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 1,
        }),
    })
    .unwrap();
    mgr.append(make_msg_entry("u2", "latest request")).unwrap();
    mgr.append(SessionEntry::Compaction {
        id: "c1".into(),
        parent_id: None,
        summary: "Compaction summary of earlier work".into(),
        first_kept_id: "u2".into(),
        tokens_before: 100,
        tokens_after: 40,
    })
    .unwrap();
    mgr.append(SessionEntry::Message {
        id: "a2".into(),
        parent_id: None,
        message: Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "follow-up answer".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 2,
        }),
    })
    .unwrap();

    let raw = mgr.get_messages();
    assert_eq!(raw.len(), 4);

    let active = mgr.get_active_messages();
    assert_eq!(active.len(), 3);
    match &active[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => {
                assert_eq!(text, "Compaction summary of earlier work")
            }
            other => panic!("unexpected summary content: {other:?}"),
        },
        other => panic!("unexpected active message: {other:?}"),
    }
    match &active[1] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "latest request"),
            other => panic!("unexpected kept user content: {other:?}"),
        },
        other => panic!("unexpected kept message: {other:?}"),
    }
}

#[test]
fn session_compaction_active_messages_fall_back_to_raw_when_first_kept_missing() {
    let mut mgr = SessionManager::in_memory();
    mgr.append(make_msg_entry("u1", "hello")).unwrap();
    mgr.append(SessionEntry::Compaction {
        id: "c1".into(),
        parent_id: None,
        summary: "summary only".into(),
        first_kept_id: "missing".into(),
        tokens_before: 10,
        tokens_after: 3,
    })
    .unwrap();

    let active = mgr.get_active_messages();
    assert_eq!(active.len(), 1);
    match &active[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "summary only"),
            other => panic!("unexpected summary-only content: {other:?}"),
        },
        other => panic!("unexpected active message: {other:?}"),
    }
}

#[test]
fn session_compaction_fork_preserves_compacted_branch_semantics() {
    let tmp = TempDir::new().unwrap();
    let fork_path = tmp.path().join("forked.jsonl");

    let mut mgr = SessionManager::in_memory();
    mgr.append(make_msg_entry("u1", "older")).unwrap();
    mgr.append(make_msg_entry("u2", "newer")).unwrap();
    mgr.append(SessionEntry::Compaction {
        id: "c1".into(),
        parent_id: None,
        summary: "summary older".into(),
        first_kept_id: "u2".into(),
        tokens_before: 20,
        tokens_after: 8,
    })
    .unwrap();
    mgr.append(SessionEntry::Message {
        id: "a2".into(),
        parent_id: None,
        message: Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 3,
        }),
    })
    .unwrap();

    let forked = mgr.fork("a2", &fork_path).unwrap();
    let active = forked.get_active_messages();
    assert_eq!(active.len(), 3);
    match &active[0] {
        Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "summary older"),
            other => panic!("unexpected summary content: {other:?}"),
        },
        other => panic!("unexpected active message: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn session_jsonl_is_private_on_disk() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let mut mgr = SessionManager::new(Path::new("/tmp"), tmp.path()).unwrap();
    mgr.append_tool_result_message(ToolResultMessage {
        tool_call_id: "call-private".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "private prompt".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    })
    .unwrap();

    let mode = std::fs::metadata(mgr.path().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn append_tool_result_message_persists_redacted_content() {
    let tmp = tempfile::tempdir().unwrap();
    let mut mgr = SessionManager::new(Path::new("/tmp"), tmp.path()).unwrap();
    mgr.append_tool_result_message(ToolResultMessage {
        tool_call_id: "call-redacted".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "[REDACTED:test-service]".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    })
    .unwrap();

    let path = mgr.path().unwrap();
    let contents = std::fs::read_to_string(path).unwrap();
    assert!(contents.contains("[REDACTED:test-service]"));
    assert!(!contents.contains("native-secret-value"));
}

#[test]
fn session_create_append_reopen() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    mgr.append(make_msg_entry("m1", "hello")).unwrap();
    mgr.append(make_msg_entry("m2", "world")).unwrap();
    mgr.append(make_msg_entry("m3", "!")).unwrap();

    let path = mgr.path().unwrap().to_path_buf();
    assert!(path.exists());

    // Reopen and verify messages match
    let reopened = SessionManager::open(&path).unwrap();
    let original_msgs = mgr.get_messages();
    let reopened_msgs = reopened.get_messages();
    assert_eq!(original_msgs.len(), reopened_msgs.len());
    assert_eq!(reopened_msgs.len(), 3);

    // Verify parent chain: m1 has no parent, m2's parent is m1, m3's parent is m2
    let entries = reopened.entries();
    for entry in entries {
        if let SessionEntry::Message { id, parent_id, .. } = entry {
            match id.as_str() {
                "m1" => assert_eq!(*parent_id, None),
                "m2" => assert_eq!(parent_id.as_deref(), Some("m1")),
                "m3" => assert_eq!(parent_id.as_deref(), Some("m2")),
                _ => {}
            }
        }
    }
}

#[test]
fn session_branch() {
    let mut mgr = SessionManager::in_memory();
    // Append 5 messages (m1..m5)
    for i in 1..=5 {
        mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
            .unwrap();
    }
    assert_eq!(mgr.get_messages().len(), 5);
    assert_eq!(mgr.leaf_id(), Some("m5"));

    // Navigate back to m3
    mgr.navigate("m3").unwrap();
    assert_eq!(mgr.leaf_id(), Some("m3"));

    // Append 2 new messages on the branch
    mgr.append(make_msg_entry("b1", "branch 1")).unwrap();
    mgr.append(make_msg_entry("b2", "branch 2")).unwrap();

    // get_branch should return: header-less chain of m1, m2, m3, b1, b2
    let branch = mgr.get_branch();
    let branch_ids: Vec<Option<&str>> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(
        branch_ids,
        vec![Some("m1"), Some("m2"), Some("m3"), Some("b1"), Some("b2")]
    );
    assert_eq!(mgr.get_messages().len(), 5);

    // Navigate back to m5 to verify original branch still works
    mgr.navigate("m5").unwrap();
    let main_branch = mgr.get_branch();
    let main_ids: Vec<Option<&str>> = main_branch.iter().map(|e| e.id()).collect();
    assert_eq!(
        main_ids,
        vec![Some("m1"), Some("m2"), Some("m3"), Some("m4"), Some("m5")]
    );
}

#[test]
fn session_fork() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    for i in 1..=5 {
        mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
            .unwrap();
    }

    let fork_path = session_dir.join("forked.jsonl");
    let forked = mgr.fork("m3", &fork_path).unwrap();

    // Forked session should have header + m1, m2, m3
    assert_eq!(forked.get_messages().len(), 3);
    assert_eq!(forked.leaf_id(), Some("m3"));
    assert!(fork_path.exists());

    // Reopen the forked file and verify
    let reopened = SessionManager::open(&fork_path).unwrap();
    assert_eq!(reopened.get_messages().len(), 3);
}

#[test]
fn session_list() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    // Create two sessions
    let mut s1 = SessionManager::new(&cwd, &session_dir).unwrap();
    s1.append(make_msg_entry("a1", "first session")).unwrap();
    s1.set_name("First");

    let mut s2 = SessionManager::new(&cwd, &session_dir).unwrap();
    s2.append(make_msg_entry("b1", "second session")).unwrap();
    s2.append(make_msg_entry("b2", "more stuff")).unwrap();
    s2.set_summary("Second session summary");

    let sessions = SessionManager::list(&session_dir).unwrap();
    assert_eq!(sessions.len(), 2);

    // Both should have the right cwd
    for s in &sessions {
        assert_eq!(s.cwd, cwd.to_string_lossy().to_string());
    }

    // One has 1 message, the other has 2
    let mut counts: Vec<usize> = sessions.iter().map(|s| s.message_count).collect();
    counts.sort();
    assert_eq!(counts, vec![1, 2]);

    // first_message and last_message should be set
    for s in &sessions {
        assert!(s.first_message.is_some());
        assert!(s.last_message.is_some());
    }

    assert!(sessions.iter().any(|s| s.name.as_deref() == Some("First")));
    assert!(sessions
        .iter()
        .any(|s| s.summary.as_deref() == Some("Second session summary")));
}

#[test]
fn session_list_page_orders_by_updated_time_and_limits() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    let mut old = SessionManager::new(&cwd, &session_dir).unwrap();
    old.append(make_msg_entry("old", "old session")).unwrap();
    let _old_path = old.path().unwrap().to_path_buf();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut new = SessionManager::new(&cwd, &session_dir).unwrap();
    new.append(make_msg_entry("new", "new session")).unwrap();
    let new_path = new.path().unwrap().to_path_buf();

    let sessions = SessionManager::list_page(&session_dir, 0, 1, None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].path, new_path);
}

#[test]
fn session_resumable_list_page_skips_empty_sessions_before_applying_offset() {
    let tmp = TempDir::new().unwrap();
    let current_dir = tmp.path().join("current");
    let legacy_dir = tmp.path().join("legacy");
    let cwd = tmp.path().join("project");

    let mut empty_content = SessionManager::new(&cwd, &current_dir).unwrap();
    empty_content
        .append(make_msg_entry("empty-content", "   "))
        .unwrap();
    let mut current = SessionManager::new(&cwd, &current_dir).unwrap();
    current
        .append(make_msg_entry("current", "current session"))
        .unwrap();
    let mut legacy = SessionManager::new(&cwd, &legacy_dir).unwrap();
    legacy
        .append(make_msg_entry("legacy", "legacy session"))
        .unwrap();

    let first = SessionManager::list_resumable_page_from_dirs(
        &[current_dir.clone(), legacy_dir.clone()],
        0,
        1,
        None,
    )
    .unwrap();
    let second =
        SessionManager::list_resumable_page_from_dirs(&[current_dir, legacy_dir], 1, 1, None)
            .unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].id, second[0].id);
    assert!(first[0].message_count > 0);
    assert!(second[0].message_count > 0);
}

#[test]
fn session_list_captures_last_message() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    let mut session = SessionManager::new(&cwd, &session_dir).unwrap();
    session
        .append(make_msg_entry("first", "first prompt"))
        .unwrap();
    session
        .append(make_msg_entry("last", "latest response"))
        .unwrap();

    let sessions = SessionManager::list(&session_dir).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].first_message.as_deref(), Some("first prompt"));
    assert_eq!(sessions[0].last_message.as_deref(), Some("latest response"));
}

#[test]
fn session_list_includes_header_only_sessions() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();

    let mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let id = mgr.session_id().unwrap();

    let sessions = SessionManager::list(&session_dir).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
    assert_eq!(sessions[0].message_count, 0);
    assert!(sessions[0].first_message.is_none());
}

#[test]
fn session_continue_recent() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd_a = tmp.path().join("project-a");
    let cwd_b = tmp.path().join("project-b");

    // Create a session for cwd_a
    let mut s1 = SessionManager::new(&cwd_a, &session_dir).unwrap();
    s1.append(make_msg_entry("a1", "hello from a")).unwrap();

    // Create a session for cwd_b
    let mut s2 = SessionManager::new(&cwd_b, &session_dir).unwrap();
    s2.append(make_msg_entry("b1", "hello from b")).unwrap();

    // continue_recent for cwd_a should find s1
    let continued = SessionManager::continue_recent(&cwd_a, &session_dir)
        .unwrap()
        .expect("should find a session");
    assert_eq!(continued.get_messages().len(), 1);

    // continue_recent for a non-existent cwd returns None
    let none = SessionManager::continue_recent(Path::new("/nonexistent"), &session_dir).unwrap();
    assert!(none.is_none());
}

#[test]
fn session_name_and_summary_persist_across_reopen() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");

    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    mgr.append(make_msg_entry("m1", "hello world")).unwrap();
    mgr.set_name("Debug auth");
    mgr.set_summary("Investigating OAuth login failures");

    let path = mgr.path().unwrap().to_path_buf();
    let reopened = SessionManager::open(&path).unwrap();
    assert_eq!(reopened.name(), Some("Debug auth"));
    assert_eq!(
        reopened.summary(),
        Some("Investigating OAuth login failures")
    );
}

#[test]
fn session_in_memory() {
    let mut mgr = SessionManager::in_memory();
    assert!(mgr.path().is_none());

    mgr.append(make_msg_entry("m1", "hello")).unwrap();
    mgr.append(make_msg_entry("m2", "world")).unwrap();

    assert_eq!(mgr.get_messages().len(), 2);
    assert_eq!(mgr.entries().len(), 2);
}

#[test]
fn session_malformed_jsonl() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("bad.jsonl");

    // Write a file with a mix of valid and invalid lines
    let content = format!(
        "{}\n\
             NOT VALID JSON\n\
             {}\n\
             {{\"type\":\"unknown_variant\",\"foo\":1}}\n\
             {}\n",
        serde_json::to_string(&SessionEntry::Header {
            version: 1,
            created_at: 1000,
            cwd: "/tmp".into(),
        })
        .unwrap(),
        serde_json::to_string(&SessionEntry::Message {
            id: "m1".into(),
            parent_id: None,
            message: Message::user("hello"),
        })
        .unwrap(),
        serde_json::to_string(&SessionEntry::Message {
            id: "m2".into(),
            parent_id: Some("m1".into()),
            message: Message::user("world"),
        })
        .unwrap(),
    );
    std::fs::write(&path, content).unwrap();

    // Should succeed, skipping the bad lines
    let mgr = SessionManager::open(&path).unwrap();
    // Header + 2 valid messages (bad lines skipped)
    assert_eq!(mgr.entries().len(), 3);
    assert_eq!(mgr.get_messages().len(), 2);
}

#[test]
fn session_get_tree() {
    let mut mgr = SessionManager::in_memory();
    for i in 1..=3 {
        mgr.append(make_msg_entry(&format!("m{i}"), &format!("msg {i}")))
            .unwrap();
    }
    // Branch from m2
    mgr.navigate("m2").unwrap();
    mgr.append(make_msg_entry("b1", "branch")).unwrap();

    let tree = mgr.get_tree();
    // Root should be m1 (no parent)
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].entry.id(), Some("m1"));

    // m1 -> m2
    assert_eq!(tree[0].children.len(), 1);
    let m2_node = &tree[0].children[0];
    assert_eq!(m2_node.entry.id(), Some("m2"));

    // m2 has two children: m3 and b1
    assert_eq!(m2_node.children.len(), 2);
    let child_ids: Vec<Option<&str>> = m2_node.children.iter().map(|n| n.entry.id()).collect();
    assert!(child_ids.contains(&Some("m3")));
    assert!(child_ids.contains(&Some("b1")));
}

#[test]
fn append_assistant_turn_persists_canonical_usage_once() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("sessions");
    let cwd = tmp.path().join("project");
    let model = make_test_model();

    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let message = AssistantMessage {
        content: vec![imp_llm::ContentBlock::Text {
            text: "done".into(),
        }],
        usage: Some(imp_llm::Usage {
            input_tokens: 100,
            output_tokens: 25,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
        }),
        stop_reason: imp_llm::StopReason::EndTurn,
        timestamp: 123,
    };

    let (_assistant_id, usage_id) = mgr
        .append_assistant_turn(&model, 3, message.clone())
        .unwrap();
    assert!(usage_id.is_some());

    let (_assistant_id_2, usage_id_2) = mgr
        .append_assistant_turn(
            &model,
            4,
            AssistantMessage {
                usage: None,
                ..message
            },
        )
        .unwrap();
    assert!(usage_id_2.is_none());

    let usage_records = mgr.usage_records();
    assert_eq!(usage_records.len(), 1);
    assert_eq!(usage_records[0].turn_index, Some(3));
    assert_eq!(usage_records[0].provider.as_deref(), Some("test-provider"));
    assert_eq!(usage_records[0].model.as_deref(), Some("test-model"));
}

#[test]
fn append_checkpoint_record_round_trips_and_lookup_works() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();

    let record = SessionCheckpointRecord {
        version: CHECKPOINT_RECORD_VERSION,
        checkpoint_id: "cp-1".into(),
        created_at: 123,
        label: Some("before edits".into()),
        files: vec!["src/main.rs".into(), "src/lib.rs".into()],
    };
    mgr.append_checkpoint_record(record.clone()).unwrap();

    assert_eq!(mgr.checkpoint_records(), vec![record.clone()]);
    assert_eq!(
        mgr.find_checkpoint_record("cp-1").unwrap().label.as_deref(),
        Some("before edits")
    );
    assert_eq!(
        mgr.find_checkpoint_record("before edits")
            .unwrap()
            .checkpoint_id,
        "cp-1"
    );
}

#[test]
fn restore_checkpoint_uses_checkpoint_state() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&cwd).unwrap();
    let file = cwd.join("main.rs");
    std::fs::write(&file, "original").unwrap();

    let checkpoint_state = crate::tools::CheckpointState::new();
    let checkpoint = checkpoint_state
        .snapshot_paths(std::slice::from_ref(&file), Some("before edits".into()))
        .unwrap()
        .unwrap();
    std::fs::write(&file, "modified").unwrap();

    let mut mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    mgr.append_checkpoint_record(SessionCheckpointRecord {
        version: CHECKPOINT_RECORD_VERSION,
        checkpoint_id: checkpoint.id.clone(),
        created_at: checkpoint.created_at,
        label: checkpoint.label.clone(),
        files: checkpoint
            .files
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    })
    .unwrap();

    let restored = mgr
        .restore_checkpoint(&checkpoint_state, "before edits")
        .unwrap();
    assert_eq!(restored, vec![file.clone()]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
}

#[test]
fn session_navigate_invalid() {
    let mut mgr = SessionManager::in_memory();
    mgr.append(make_msg_entry("m1", "hello")).unwrap();

    let result = mgr.navigate("nonexistent");
    assert!(result.is_err());
}
