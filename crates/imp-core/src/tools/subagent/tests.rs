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
