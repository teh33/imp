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
