use super::*;
use crate::compaction::checkpoint::{checkpoint_source, CompactionCheckpoint, CHECKPOINT_VERSION};
use crate::compaction::state::{
    extract_continuation_state, ContinuationState, FactKind, StateFact, CONTINUATION_STATE_VERSION,
};
use crate::session::SessionEntry;
use imp_llm::{AssistantMessage, StopReason};

fn append_user(session: &mut SessionManager, id: &str, text: &str) {
    session
        .append(SessionEntry::Message {
            id: id.to_string(),
            parent_id: None,
            message: Message::user(text),
        })
        .unwrap();
}

fn checkpoint(ids: &[&str], summary: &str) -> CompactionCheckpoint {
    CompactionCheckpoint {
        version: CHECKPOINT_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        previous_checkpoint_id: None,
        covered_entry_ids: ids.iter().map(|id| (*id).to_string()).collect(),
        source_fingerprint: "test-fingerprint".into(),
        continuation: ContinuationState {
            version: CONTINUATION_STATE_VERSION,
            facts: vec![StateFact {
                id: "fact-1".into(),
                kind: FactKind::Constraint,
                text: "Never lose the required constraint.".into(),
                source_entry_id: ids[0].to_string(),
                required: true,
            }],
        },
        summary: summary.into(),
        model_id: "gpt-5.6-luna".into(),
        provider_id: "openai".into(),
        thinking: "xhigh".into(),
        source_tokens: 128_000,
    }
}

#[test]
fn continuation_state_requires_visible_assistant_tool_calls() {
    let entry = ActiveSessionMessage {
        source: ActiveMessageSource::Message {
            entry_id: "assistant-1".into(),
        },
        message: Message::Assistant(AssistantMessage {
            content: vec![imp_llm::ContentBlock::ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "cargo nextest run"}),
            }],
            usage: None,
            stop_reason: StopReason::ToolUse,
            timestamp: 1,
        }),
    };

    let state = extract_continuation_state(&[entry]);

    assert_eq!(state.facts.len(), 1);
    assert!(state.facts[0].required);
    assert!(state.facts[0].text.contains("cargo nextest run"));
}

#[test]
fn activating_checkpoint_persists_v2_and_keeps_uncovered_tail() {
    let mut session = SessionManager::in_memory();
    append_user(&mut session, "one", &"old context ".repeat(2_000));
    append_user(&mut session, "two", &"more old context ".repeat(2_000));
    append_user(&mut session, "three", "recent unchanged tail");

    let result =
        activate_checkpoint(&mut session, &checkpoint(&["one", "two"], "Luna summary")).unwrap();

    assert_eq!(result.first_kept_id, "three");
    assert!(result.tokens_after < result.tokens_before);
    let active = serde_json::to_string(&session.get_active_messages()).unwrap();
    assert!(active.contains("Luna summary"));
    assert!(active.contains("Never lose the required constraint"));
    assert!(active.contains("recent unchanged tail"));
    assert!(!active.contains("more old context"));
    assert!(matches!(
        session.latest_compaction(),
        Some(SessionEntry::CompactionV2 { .. })
    ));
}

#[test]
fn activating_stale_checkpoint_leaves_session_unchanged() {
    let mut session = SessionManager::in_memory();
    append_user(&mut session, "one", &"old context ".repeat(2_000));
    append_user(&mut session, "two", "recent tail");
    let before = serde_json::to_string(&session.get_active_messages()).unwrap();

    let error =
        activate_checkpoint(&mut session, &checkpoint(&["different"], "summary")).unwrap_err();

    assert!(error.to_string().contains("active branch prefix"));
    assert_eq!(
        serde_json::to_string(&session.get_active_messages()).unwrap(),
        before
    );
    assert!(session.latest_compaction().is_none());
}

#[test]
fn v2_checkpoint_activation_survives_session_reload() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("repo");
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut session = SessionManager::new(&cwd, &sessions).unwrap();
    append_user(&mut session, "one", &"old context ".repeat(2_000));
    append_user(&mut session, "two", &"middle context ".repeat(2_000));
    append_user(&mut session, "three", "preserved tail");
    activate_checkpoint(
        &mut session,
        &checkpoint(&["one", "two"], "restart summary"),
    )
    .unwrap();
    let path = session.path().unwrap().to_path_buf();

    let reopened = SessionManager::open(&path).unwrap();
    let active = serde_json::to_string(&reopened.get_active_messages()).unwrap();

    assert!(active.contains("restart summary"));
    assert!(active.contains("preserved tail"));
    assert!(!active.contains("middle context"));
    assert!(matches!(
        reopened.latest_compaction(),
        Some(SessionEntry::CompactionV2 { .. })
    ));
}

#[test]
fn repeated_checkpoint_activation_survives_restart_each_cycle() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("repo");
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut session = SessionManager::new(&cwd, &sessions).unwrap();
    let path = session.path().unwrap().to_path_buf();

    for cycle in 1..=3 {
        let old_id = format!("cycle-{cycle}-old");
        let tail_id = format!("cycle-{cycle}-tail");
        append_user(
            &mut session,
            &old_id,
            &format!("{} old context ", cycle).repeat(2_000),
        );
        append_user(&mut session, &tail_id, &format!("tail {cycle}"));
        let active_entries = session.get_active_message_entries();
        let covered_ids = active_entries
            .iter()
            .take(active_entries.len().saturating_sub(1))
            .map(|entry| match &entry.source {
                ActiveMessageSource::Compaction { entry_id, .. }
                | ActiveMessageSource::Message { entry_id } => entry_id.clone(),
            })
            .collect::<Vec<_>>();
        let covered = covered_ids.iter().map(String::as_str).collect::<Vec<_>>();
        let mut next = checkpoint(&covered, &format!("summary {cycle}"));
        next.continuation.facts[0].text = format!("constraint through cycle {cycle}");
        activate_checkpoint(&mut session, &next).unwrap();

        session = SessionManager::open(&path).unwrap();
        let active = serde_json::to_string(&session.get_active_messages()).unwrap();
        assert!(active.contains(&format!("summary {cycle}")), "{active}");
        assert!(active.contains(&format!("tail {cycle}")), "{active}");
        assert!(
            active.contains(&format!("constraint through cycle {cycle}")),
            "{active}"
        );
    }

    let raw = serde_json::to_string(&session.get_messages()).unwrap();
    for cycle in 1..=3 {
        assert!(raw.contains(&format!("{cycle} old context")), "{raw}");
        assert!(raw.contains(&format!("tail {cycle}")), "{raw}");
    }
}

#[test]
fn repeated_checkpoint_activation_uses_stable_active_entry_ids() {
    let mut session = SessionManager::in_memory();
    append_user(&mut session, "one", &"old context ".repeat(2_000));
    append_user(&mut session, "two", &"middle context ".repeat(2_000));
    append_user(&mut session, "three", "first tail");
    activate_checkpoint(&mut session, &checkpoint(&["one", "two"], "first summary")).unwrap();

    let source = checkpoint_source(&session.get_active_message_entries(), None).unwrap();
    assert_eq!(source.uncovered_start, 1);
    assert_eq!(
        source
            .inherited_continuation
            .as_ref()
            .unwrap()
            .facts
            .first()
            .unwrap()
            .text,
        "Never lose the required constraint."
    );

    append_user(&mut session, "four", &"new context ".repeat(2_000));
    append_user(&mut session, "five", "second tail");
    let active_ids = session
        .get_active_message_entries()
        .into_iter()
        .take(3)
        .map(|entry| match entry.source {
            ActiveMessageSource::Compaction { entry_id, .. }
            | ActiveMessageSource::Message { entry_id } => entry_id,
        })
        .collect::<Vec<_>>();
    let ids = active_ids.iter().map(String::as_str).collect::<Vec<_>>();

    let result = activate_checkpoint(&mut session, &checkpoint(&ids, "second summary")).unwrap();

    assert_eq!(result.first_kept_id, "five");
    let active = serde_json::to_string(&session.get_active_messages()).unwrap();
    assert!(active.contains("second summary"));
    assert!(active.contains("second tail"));
    assert!(!active.contains("first summary"));
    assert!(!active.contains("new context"));
}
