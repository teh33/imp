use super::*;

#[test]
fn subagent_tool_schema_exposes_lifecycle_actions() {
    let tool = SubagentTool::new();
    let actions = &tool.parameters()["properties"]["action"]["enum"];
    assert_eq!(
        actions,
        &json!(["launch", "status", "wait", "send", "cancel"])
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
