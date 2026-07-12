use super::*;

#[test]
fn subagent_tool_schema_exposes_lifecycle_actions() {
    let tool = SubagentTool::new();
    let actions = &tool.parameters()["properties"]["action"]["enum"];
    assert_eq!(
        actions,
        &json!([
            "launch",
            "status",
            "wait",
            "send",
            "cancel",
            "ready",
            "integrate"
        ])
    );
}

#[test]
fn subagent_ids_reject_path_traversal() {
    assert!(valid_id("child_1").is_ok());
    assert!(valid_id("../child").is_err());
    assert!(valid_id("").is_err());
}

#[test]
fn explicit_subagent_model_overrides_configured_default() {
    let tool = SubagentTool::with_default_model(Some("default-child".into()));
    assert_eq!(
        tool.resolved_model(Some("explicit-child".into())).unwrap(),
        "explicit-child"
    );
}

#[test]
fn configured_subagent_model_fills_missing_contract_model() {
    let tool = SubagentTool::with_default_model(Some("default-child".into()));
    assert_eq!(tool.resolved_model(None).unwrap(), "default-child");
}

fn sample_input(writable_paths: Vec<PathBuf>) -> SubagentInput {
    use crate::agent::{
        ParentRunId, SubagentContext, SubagentMergePolicy, SubagentResourceLimits, SubagentRole,
    };
    SubagentInput {
        parent_run_id: ParentRunId::new("parent-1"),
        child_run_id: SubagentRunId::new("child_1"),
        model: Some("child-model".into()),
        role: SubagentRole::Implementer,
        objective: "Implement the bounded change".into(),
        context: SubagentContext::default(),
        resource_limits: SubagentResourceLimits {
            writable_paths,
            ..SubagentResourceLimits::default()
        },
        merge_policy: SubagentMergePolicy::Verify,
        output_contract: None,
    }
}

#[test]
fn read_only_subagents_do_not_allocate_workspaces() {
    assert!(workspace_request(&sample_input(Vec::new())).is_none());
}

#[test]
fn writable_subagents_get_stable_owned_workspace_requests() {
    let request = workspace_request(&sample_input(vec![PathBuf::from("src/lib.rs")])).unwrap();
    assert_eq!(request.id.as_deref(), Some("subagent-child-1"));
    assert_eq!(request.run_id, "child_1");
    assert_eq!(
        request.task.as_deref(),
        Some("Implement the bounded change")
    );
    assert!(request.base_ref.is_none());
}

fn record_with_workspace(status: imp_subagent::Status) -> imp_subagent::Record {
    imp_subagent::Record {
        version: 1,
        parent_id: "parent-1".into(),
        child_id: "child-1".into(),
        model: Some("child-model".into()),
        cwd: PathBuf::from("/tmp/workspace"),
        executable: PathBuf::from("imp"),
        worker_executable: PathBuf::from("imp"),
        child_args: Vec::new(),
        environment: Vec::new(),
        socket: PathBuf::from("/tmp/child.sock"),
        artifacts: imp_subagent::ArtifactPaths {
            state: PathBuf::from("state.json"),
            transcript: PathBuf::from("transcript.jsonl"),
            stderr: PathBuf::from("stderr.log"),
            session: PathBuf::from("session.jsonl"),
        },
        status,
        session_id: "session-1".into(),
        turn_id: None,
        summary: None,
        diagnostics: Vec::new(),
        timeout_seconds: None,
        metadata: serde_json::json!({"managed_workspace_id": "subagent-child-1"}),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

#[test]
fn durable_record_resolves_managed_workspace_id() {
    let record = record_with_workspace(imp_subagent::Status::Running);
    assert_eq!(managed_workspace_id(&record), Some("subagent-child-1"));
}

#[test]
fn promotion_requires_success_and_a_managed_workspace() {
    let running = record_with_workspace(imp_subagent::Status::Running);
    assert!(promotable_workspace_id(&running)
        .unwrap_err()
        .to_string()
        .contains("requires terminal success"));

    let mut workspace_free = record_with_workspace(imp_subagent::Status::Success);
    workspace_free.metadata = serde_json::json!({});
    assert!(promotable_workspace_id(&workspace_free)
        .unwrap_err()
        .to_string()
        .contains("no managed workspace"));
}

#[test]
fn successful_workspace_can_be_promoted_explicitly() {
    let record = record_with_workspace(imp_subagent::Status::Success);
    assert_eq!(
        promotable_workspace_id(&record).unwrap(),
        "subagent-child-1"
    );
}

#[test]
fn terminal_workspace_diagnostic_includes_status_and_child_diagnostics() {
    let mut record = record_with_workspace(imp_subagent::Status::Failed);
    record.diagnostics = vec!["provider disconnected".into(), "retry exhausted".into()];
    assert_eq!(
        terminal_workspace_diagnostic(&record),
        "subagent finished with status failed: provider disconnected; retry exhausted"
    );
}
