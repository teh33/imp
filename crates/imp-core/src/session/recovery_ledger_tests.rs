use super::*;
use crate::agent::{RecoveryCheckpoint, RecoveryCheckpointKind};

#[test]
fn recovery_checkpoint_entry_redacts_tool_args_and_output() {
    let raw_secret_arg = "super-secret-token";
    let raw_tool_output = "sensitive command output";
    let checkpoint = RecoveryCheckpoint {
        version: 1,
        turn: 3,
        kind: RecoveryCheckpointKind::ToolExecutionStart,
        tool_call_id: Some("call_123".to_string()),
        tool_name: Some("bash".to_string()),
        args_hash: Some("0123456789abcdef".to_string()),
        success: None,
        error_class: None,
        timestamp: 42,
    };

    let entry = recovery_checkpoint_entry("recovery-1", checkpoint).unwrap();
    let encoded = serde_json::to_string(&entry).unwrap();

    assert!(encoded.contains("recovery-checkpoint"));
    assert!(encoded.contains("tool_execution_start"));
    assert!(encoded.contains("0123456789abcdef"));
    assert!(!encoded.contains(raw_secret_arg));
    assert!(!encoded.contains(raw_tool_output));
}

#[test]
fn append_recovery_checkpoint_persists_redacted_custom_entry() {
    let mut session = SessionManager::in_memory();
    let checkpoint = RecoveryCheckpoint {
        version: 1,
        turn: 1,
        kind: RecoveryCheckpointKind::ToolExecutionEnd,
        tool_call_id: Some("call_456".to_string()),
        tool_name: Some("edit".to_string()),
        args_hash: Some("abcdef0123456789".to_string()),
        success: Some(true),
        error_class: None,
        timestamp: 99,
    };

    let entry_id = session.append_recovery_checkpoint(checkpoint).unwrap();

    assert!(!entry_id.is_empty());
    let entry = session.entries().last().expect("recovery checkpoint entry");
    let SessionEntry::Custom {
        custom_type, data, ..
    } = entry
    else {
        panic!("expected custom recovery checkpoint entry");
    };
    assert_eq!(custom_type, RECOVERY_CHECKPOINT_CUSTOM_TYPE);
    assert_eq!(data["kind"], "tool_execution_end");
    assert_eq!(data["tool_name"], "edit");
    assert_eq!(data["args_hash"], "abcdef0123456789");
    let encoded = serde_json::to_string(data).unwrap();
    assert!(!encoded.contains("oldText"));
    assert!(!encoded.contains("newText"));
}

#[test]
fn recovery_checkpoints_round_trip_into_ledger() {
    let mut session = SessionManager::in_memory();
    let checkpoint = RecoveryCheckpoint {
        version: 1,
        turn: 7,
        kind: RecoveryCheckpointKind::ToolPlanCreated,
        tool_call_id: Some("call_789".to_string()),
        tool_name: Some("read".to_string()),
        args_hash: Some("hash789".to_string()),
        success: Some(true),
        error_class: None,
        timestamp: 100,
    };

    session.append_recovery_checkpoint(checkpoint).unwrap();

    let checkpoints = session.recovery_checkpoints();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].tool_call_id.as_deref(), Some("call_789"));

    let reconciliation = session.recovery_ledger().reconcile_turn(7);
    assert_eq!(reconciliation.retryable_incomplete_tools.len(), 1);
    assert!(reconciliation.unsafe_incomplete_tools.is_empty());
}
