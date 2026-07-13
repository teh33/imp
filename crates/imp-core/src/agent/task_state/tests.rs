use super::*;
use imp_llm::{ContentBlock, Message, ToolResultMessage};
use std::path::Path;

fn result(tool: &str, details: serde_json::Value, is_error: bool) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: "call".into(),
        tool_name: tool.into(),
        content: vec![ContentBlock::Text {
            text: String::new(),
        }],
        is_error,
        details,
        timestamp: 0,
    }
}

#[test]
fn disabled_state_does_not_project_or_block_closeout() {
    let mut state = SessionTaskState::new("Fix bug");
    state.replace_plan(vec!["Run tests".into()]).unwrap();
    state.disable();
    assert!(!state.is_enabled());
    assert!(state.closeout_issues().is_empty());
    assert_eq!(state.project_messages(&[Message::user("hello")]).len(), 1);
}

#[test]
fn ordinary_one_shot_prompt_keeps_planning_dormant() {
    let mut state = SessionTaskState::new("Initial");
    state.begin_prompt("Fix slugify and run its focused tests");
    assert!(!state.planning_active());
    assert!(!state.should_project());
}

#[test]
fn explicit_planning_prompt_activates_planning() {
    let mut state = SessionTaskState::new("Initial");
    state.begin_prompt("Make a plan for this multi-step migration");
    assert!(state.planning_active());
    assert!(state.should_project());
}

#[test]
fn begin_prompt_preserves_incomplete_task_across_turns() {
    let mut state = SessionTaskState::new("Fix bug");
    state.replace_plan(vec!["Run tests".into()]).unwrap();

    state.begin_prompt("Are you done?");

    assert_eq!(state.objective, "Fix bug");
    assert_eq!(state.steps.len(), 1);
}

#[test]
fn begin_prompt_replaces_completed_task() {
    let mut state = SessionTaskState::new("Fix bug");
    state.replace_plan(vec!["Run tests".into()]).unwrap();
    state
        .update_step(1, TaskStepStatus::Completed, None)
        .unwrap();

    state.begin_prompt("Explain the parser");

    assert_eq!(state.objective, "Explain the parser");
    assert!(state.steps.is_empty());
}

#[test]
fn edit_records_path_and_requires_verification() {
    let mut state = SessionTaskState::new("Fix bug");
    state.record_tool_result(
        &result(
            "edit",
            serde_json::json!({"path": "/repo/src/lib.rs", "dry_run": false}),
            false,
        ),
        Path::new("/repo"),
    );
    assert!(state.changed_paths.contains("src/lib.rs"));
    assert!(state.verification_required);
}

#[test]
fn runtime_complexity_activates_planning() {
    let mut state = SessionTaskState::new("Migrate modules");
    for path in ["a.rs", "b.rs", "c.rs"] {
        state.record_tool_result(
            &result("edit", serde_json::json!({"path": path}), false),
            Path::new("/repo"),
        );
    }
    assert!(state.planning_active());
}

#[test]
fn repeated_failures_activate_planning() {
    let mut state = SessionTaskState::new("Fix bug");
    for command in ["cargo test one", "cargo test two"] {
        state.record_tool_result(
            &result(
                "bash",
                serde_json::json!({"command": command, "exit_code": 1}),
                true,
            ),
            Path::new("/repo"),
        );
    }
    assert!(state.planning_active());
}

#[test]
fn successful_command_resolves_verification_requirement() {
    let mut state = SessionTaskState::new("Fix bug");
    state.verification_required = true;
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "cargo test", "exit_code": 0}),
            false,
        ),
        Path::new("/repo"),
    );
    assert!(!state.verification_required);
    assert!(state.failures.is_empty());
}

#[test]
fn unrelated_successful_command_does_not_resolve_verification() {
    let mut state = SessionTaskState::new("Fix bug");
    state.verification_required = true;
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "git status", "exit_code": 0}),
            false,
        ),
        Path::new("/repo"),
    );
    assert!(state.verification_required);
}

#[test]
fn managed_job_lifecycle_does_not_create_task_evidence() {
    let mut state = SessionTaskState::new("Fix bug");
    state.record_tool_result(
        &ToolResultMessage {
            tool_call_id: "job".into(),
            tool_name: "bash".into(),
            content: Vec::new(),
            is_error: false,
            details: serde_json::json!({
                "job_id": "job-1",
                "managed_job": true,
                "state": "running",
                "exit": null
            }),
            timestamp: 1,
        },
        Path::new("/repo"),
    );

    assert!(state.checks.is_empty());
    assert!(state.failures.is_empty());
    assert!(!state.verification_required);
}

#[test]
fn successful_command_resolves_unknown_command_failure() {
    let mut state = SessionTaskState::new("Fix bug");
    state.record_tool_result(
        &ToolResultMessage {
            tool_call_id: "failed".into(),
            tool_name: "bash".into(),
            content: Vec::new(),
            is_error: true,
            details: serde_json::json!({"exit_code": 1}),
            timestamp: 1,
        },
        Path::new("/repo"),
    );
    assert_eq!(state.failures, ["command failed: <unknown command>"]);

    state.record_tool_result(
        &ToolResultMessage {
            tool_call_id: "passed".into(),
            tool_name: "bash".into(),
            content: Vec::new(),
            is_error: false,
            details: serde_json::json!({"command": "true", "exit_code": 0}),
            timestamp: 2,
        },
        Path::new("/repo"),
    );

    assert!(state.failures.is_empty());
}

#[test]
fn successful_check_resolves_prior_command_failure() {
    let mut state = SessionTaskState::new("Fix bug");
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "cargo test broken", "exit_code": 1}),
            true,
        ),
        Path::new("/repo"),
    );
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "cargo test fixed", "exit_code": 0}),
            false,
        ),
        Path::new("/repo"),
    );
    assert!(state.failures.is_empty());
}

#[test]
fn successful_check_does_not_hide_unrelated_command_failure() {
    let mut state = SessionTaskState::new("Fix bug");
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "git show missing", "exit_code": 1}),
            true,
        ),
        Path::new("/repo"),
    );
    state.record_tool_result(
        &result(
            "bash",
            serde_json::json!({"command": "cargo test fixed", "exit_code": 0}),
            false,
        ),
        Path::new("/repo"),
    );
    assert_eq!(state.failures, vec!["command failed: git show missing"]);
}

#[test]
fn closeout_issues_include_pending_steps_and_verification() {
    let mut state = SessionTaskState::new("Fix bug");
    state.replace_plan(vec!["Run tests".into()]).unwrap();
    state.verification_required = true;
    let issues = state.closeout_issues();
    assert!(issues.iter().any(|issue| issue.contains("verification")));
    assert!(issues.iter().any(|issue| issue.contains("step 1")));
}

#[test]
fn closeout_blocks_done_status_when_work_remains() {
    let mut state = SessionTaskState::new("Fix bug");
    state.verification_required = true;
    let status = enforce_task_closeout(
        RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        },
        &state,
    );
    assert!(matches!(
        status,
        RunFinalStatus::Blocked {
            reason: StopReason::CloseoutIncomplete,
            ..
        }
    ));
}

#[test]
fn projection_precedes_trailing_tool_results() {
    let mut state = SessionTaskState::new("Fix bug");
    state.activate_planning();
    let messages = vec![
        Message::user("Fix it"),
        Message::ToolResult(result(
            "bash",
            serde_json::json!({"command": "cargo test", "exit_code": 0}),
            false,
        )),
    ];
    let projected = state.project_messages(&messages);
    assert!(matches!(projected[1], Message::User(_)));
    assert!(matches!(projected[2], Message::ToolResult(_)));
}

#[test]
fn planning_projection_advertises_task_tool() {
    let mut state = SessionTaskState::new("Plan migration");
    state.activate_planning();
    assert!(state.projection().contains("Use `task`"));
}

#[test]
fn projection_is_runtime_owned_and_compact() {
    let mut state = SessionTaskState::new("Fix bug");
    state
        .replace_plan(vec!["Inspect parser".into(), "Run tests".into()])
        .unwrap();
    state.add_constraint("Preserve API".into()).unwrap();
    state
        .update_step(1, TaskStepStatus::Completed, None)
        .unwrap();
    let projection = state.projection();
    assert!(projection.contains("Objective: Fix bug"));
    assert!(projection.contains("1 [completed] Inspect parser"));
    assert!(projection.contains("Constraints: Preserve API"));
}
