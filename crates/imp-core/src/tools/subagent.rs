use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::agent::{SubagentEvent, SubagentInput, SubagentSpawnResult, SubagentStatus};
use crate::error::Result;

pub struct SubagentTool;

#[derive(Debug, Deserialize)]
struct SubagentParams {
    action: String,
    input: Option<SubagentInput>,
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn label(&self) -> &str {
        "Subagent"
    }

    fn description(&self) -> &str {
        "Launch and manage bounded subagents from workflow-generated contracts. Use this when workflow.run returns a subagent_action contract."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["launch"],
                    "description": "Subagent action to perform."
                },
                "input": {
                    "type": "object",
                    "description": "Workflow-generated SubagentInput launch contract."
                }
            }
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let params: SubagentParams = serde_json::from_value(params).map_err(|error| {
            crate::error::Error::Tool(format!("invalid subagent params: {error}"))
        })?;
        match params.action.as_str() {
            "launch" => launch_subagent(
                params
                    .input
                    .ok_or_else(|| crate::error::Error::Tool("missing `input` parameter".into()))?,
                &ctx,
            ),
            other => Ok(ToolOutput::error(format!(
                "unsupported subagent action `{other}`; expected launch"
            ))),
        }
    }
}

fn launch_subagent(input: SubagentInput, ctx: &ToolContext) -> Result<ToolOutput> {
    validate_launch_input(&input, ctx)?;
    let event = SubagentEvent::Started {
        child_run_id: input.child_run_id.clone(),
        role: input.role.clone(),
        objective: input.objective.clone(),
    };
    let result = SubagentSpawnResult {
        child_run_id: input.child_run_id.clone(),
        events: vec![event.clone()],
    };
    let text = format!(
        "Subagent launched: {} [{:?}]\nObjective: {}\nStatus: {:?}",
        input.child_run_id.as_str(),
        input.role,
        input.objective,
        SubagentStatus::Running
    );
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "launch",
            "status": "running",
            "result": result,
            "input": input,
        }),
        is_error: false,
    })
}

fn validate_launch_input(input: &SubagentInput, ctx: &ToolContext) -> Result<()> {
    if input.child_run_id.as_str().trim().is_empty() {
        return Err(crate::error::Error::Tool(
            "subagent launch requires a non-empty child_run_id".into(),
        ));
    }
    if input.objective.trim().is_empty() {
        return Err(crate::error::Error::Tool(
            "subagent launch requires a non-empty objective".into(),
        ));
    }
    for path in &input.resource_limits.writable_paths {
        ctx.check_write_path(path).map_err(|reason| {
            crate::error::Error::Tool(format!("subagent launch denied: {reason}"))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use crate::agent::{
        ParentRunId, SubagentContext, SubagentMergePolicy, SubagentResourceLimits, SubagentRole,
        SubagentRunId,
    };
    use crate::config::{AgentMode, Config};
    use crate::policy::RunPolicy;
    use crate::tools::{AnchorStore, CheckpointState, FileCache, FileTracker, ToolUpdate};
    use crate::trust::Provenance;
    use crate::ui::NullInterface;
    use crate::workflow_review::TurnWorkflowReviewAccumulator;

    fn test_ctx(dir: &Path, run_policy: RunPolicy) -> ToolContext {
        let (update_tx, _) = tokio::sync::mpsc::channel::<ToolUpdate>(8);
        let (command_tx, _) = tokio::sync::mpsc::channel(8);
        ToolContext {
            cwd: dir.to_path_buf(),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx,
            command_tx,
            ui: Arc::new(NullInterface),
            file_cache: Arc::new(FileCache::new()),
            checkpoint_state: Arc::new(CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(FileTracker::new())),
            anchor_store: Arc::new(AnchorStore::new()),
            lua_tool_loader: None,
            mode: AgentMode::Full,
            read_max_lines: 500,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                TurnWorkflowReviewAccumulator::default(),
            )),
            config: Arc::new(Config::default()),
            run_policy,
            supporting_provenance: Vec::<Provenance>::new(),
        }
    }

    fn sample_input() -> SubagentInput {
        SubagentInput {
            parent_run_id: ParentRunId::new("parent-1"),
            child_run_id: SubagentRunId::new("child-1"),
            role: SubagentRole::Verifier,
            objective: "Verify workflow handoff".to_string(),
            context: SubagentContext::default(),
            resource_limits: SubagentResourceLimits {
                writable_paths: vec![PathBuf::from("artifacts/out.md")],
                ..SubagentResourceLimits::default()
            },
            merge_policy: SubagentMergePolicy::Verify,
            output_contract: Some("Report verification evidence".into()),
        }
    }

    #[tokio::test]
    async fn subagent_tool_schema_supports_launch() {
        let schema = SubagentTool.parameters();
        assert_eq!(schema["properties"]["action"]["enum"][0], "launch");
        assert!(schema["properties"].get("input").is_some());
    }

    #[tokio::test]
    async fn subagent_tool_events_returns_started_event() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let ctx = test_ctx(temp.path(), RunPolicy::default());
        let output = SubagentTool
            .execute(
                "call-1",
                json!({ "action": "launch", "input": sample_input() }),
                ctx,
            )
            .await
            .expect("launch succeeds");

        let text = output.text_content().expect("text output");
        assert!(text.contains("Subagent launched: child-1"), "{text}");
        assert_eq!(output.details["status"], "running");
        assert_eq!(output.details["result"]["child_run_id"], "child-1");
        assert_eq!(
            output.details["result"]["events"][0]["started"]["child_run_id"],
            "child-1"
        );
    }

    #[tokio::test]
    async fn subagent_tool_policy_rejects_disallowed_write_scope() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let ctx = test_ctx(temp.path(), RunPolicy::new().allow_write("allowed/**"));
        let error = match SubagentTool
            .execute(
                "call-1",
                json!({ "action": "launch", "input": sample_input() }),
                ctx,
            )
            .await
        {
            Ok(_) => panic!("policy denial should fail the tool call"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("subagent launch denied"));
    }
}
