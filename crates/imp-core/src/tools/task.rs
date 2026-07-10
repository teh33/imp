use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use imp_llm::ContentBlock;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::agent::task_state::{SessionTaskState, TaskStepStatus};
use crate::error::Result;

pub struct TaskTool {
    state: Arc<Mutex<SessionTaskState>>,
}

impl TaskTool {
    pub fn new(state: Arc<Mutex<SessionTaskState>>) -> Self {
        state.lock().expect("session task state lock").enable();
        Self { state }
    }
}

#[derive(Debug, Deserialize)]
struct TaskParams {
    action: TaskAction,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    step_id: Option<u32>,
    #[serde(default)]
    status: Option<TaskStepStatus>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    constraint: Option<String>,
    #[serde(default)]
    blocker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskAction {
    Show,
    Plan,
    UpdateStep,
    AddConstraint,
    AddBlocker,
    ResolveBlocker,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn label(&self) -> &str {
        "Task"
    }

    fn description(&self) -> &str {
        "Inspect or update the current session task plan, constraints, and blockers. Runtime-recorded file changes, command outcomes, and verification state are authoritative."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["show", "plan", "update_step", "add_constraint", "add_blocker", "resolve_blocker"]
                },
                "steps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Complete ordered plan for action=plan. Replaces prior steps."
                },
                "step_id": { "type": "integer", "minimum": 1 },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "blocked"]
                },
                "note": { "type": "string" },
                "constraint": { "type": "string" },
                "blocker": { "type": "string" }
            },
            "required": ["action"]
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let params: TaskParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "Invalid task parameters: {error}"
                )))
            }
        };
        let mut state = self.state.lock().expect("session task state lock");
        let result = apply_action(&mut state, params);
        match result {
            Ok(()) => Ok(output(&state)),
            Err(error) => Ok(ToolOutput::error(error)),
        }
    }
}

fn apply_action(
    state: &mut SessionTaskState,
    params: TaskParams,
) -> std::result::Result<(), String> {
    match params.action {
        TaskAction::Show => Ok(()),
        TaskAction::Plan => state.replace_plan(params.steps),
        TaskAction::UpdateStep => state.update_step(
            params.step_id.ok_or("step_id is required")?,
            params.status.ok_or("status is required")?,
            params.note,
        ),
        TaskAction::AddConstraint => {
            state.add_constraint(params.constraint.ok_or("constraint is required")?)
        }
        TaskAction::AddBlocker => state.add_blocker(params.blocker.ok_or("blocker is required")?),
        TaskAction::ResolveBlocker => {
            state.resolve_blocker(&params.blocker.ok_or("blocker is required")?)
        }
    }
}

fn output(state: &SessionTaskState) -> ToolOutput {
    let details = serde_json::to_value(state).unwrap_or_else(|_| json!({}));
    ToolOutput {
        content: vec![ContentBlock::Text {
            text: state.projection(),
        }],
        is_error: false,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_replaces_steps_with_stable_ids() {
        let mut state = SessionTaskState::new("Fix bug");
        apply_action(
            &mut state,
            TaskParams {
                action: TaskAction::Plan,
                steps: vec!["Inspect".into(), "Verify".into()],
                step_id: None,
                status: None,
                note: None,
                constraint: None,
                blocker: None,
            },
        )
        .unwrap();
        assert_eq!(state.steps.len(), 2);
        assert_eq!(state.steps[0].id, 1);
        assert_eq!(state.steps[1].id, 2);
    }

    #[test]
    fn task_tool_enables_runtime_ledger() {
        let state = Arc::new(Mutex::new(SessionTaskState::default()));
        assert!(!state.lock().unwrap().is_enabled());
        let tool = TaskTool::new(Arc::clone(&state));
        assert!(state.lock().unwrap().is_enabled());
        assert!(!tool.is_readonly());
    }

    #[test]
    fn model_cannot_mutate_runtime_evidence() {
        let schema = TaskTool::new(Arc::new(Mutex::new(SessionTaskState::default()))).parameters();
        assert!(schema["properties"].get("changed_paths").is_none());
        assert!(schema["properties"].get("verification_required").is_none());
        assert!(schema["properties"].get("failures").is_none());
    }
}
