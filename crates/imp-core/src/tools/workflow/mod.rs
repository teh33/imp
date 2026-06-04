use std::path::PathBuf;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::agent::SubagentInput;
use crate::error::Result;
use crate::workflow::{ValidateOptions, ValidationMode};

mod checks;
mod contracts;
mod files;
mod mutation;
mod query;
mod readiness;
mod render;
mod run;
mod status;
use checks::WorkflowCommandStepRun;
use mutation::{complete_step_action, update_action};
use query::{list_action, show_action, validate_action, workflows_root};
use run::run_action;

pub struct WorkflowTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowAction {
    List,
    Show,
    Validate,
    Run,
    CompleteStep,
    Update,
}

impl WorkflowAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Show => "show",
            Self::Validate => "validate",
            Self::Run => "run",
            Self::CompleteStep => "complete_step",
            Self::Update => "update",
        }
    }

    fn parse(value: &str) -> std::result::Result<Self, String> {
        match value {
            "list" => Ok(Self::List),
            "show" => Ok(Self::Show),
            "validate" => Ok(Self::Validate),
            "run" => Ok(Self::Run),
            "complete_step" => Ok(Self::CompleteStep),
            "update" => Ok(Self::Update),
            other => Err(format!(
                "unsupported workflow action `{other}`; expected list, show, validate, run, complete_step, or update"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowValidationModeParam {
    Draft,
    Strict,
}

impl WorkflowValidationModeParam {
    fn parse(value: Option<&str>) -> std::result::Result<Self, String> {
        match value.unwrap_or("strict") {
            "draft" => Ok(Self::Draft),
            "strict" => Ok(Self::Strict),
            other => Err(format!(
                "unsupported workflow validation mode `{other}`; expected draft or strict"
            )),
        }
    }

    fn options(self, workflow_root: PathBuf) -> ValidateOptions {
        match self {
            Self::Draft => ValidateOptions {
                mode: ValidationMode::Draft,
                workflow_root,
            },
            Self::Strict => ValidateOptions {
                mode: ValidationMode::Strict,
                workflow_root,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowListItem {
    id: String,
    title: String,
    status: String,
    kind: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowValidationResult {
    id: String,
    ok: bool,
    diagnostics: Vec<WorkflowDiagnosticView>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowDiagnosticView {
    path: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowRunResult {
    id: String,
    status: String,
    execution_mode: WorkflowExecutionMode,
    next_action: WorkflowNextAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowExecutionMode {
    MainAgent,
    Subagents,
}

impl WorkflowExecutionMode {
    fn parse(value: Option<&str>) -> std::result::Result<Self, String> {
        match value.unwrap_or("main_agent") {
            "main_agent" | "main" => Ok(Self::MainAgent),
            "subagents" | "subagent" => Ok(Self::Subagents),
            other => Err(format!(
                "unsupported workflow execution mode `{other}`; expected main_agent or subagents"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::MainAgent => "main agent",
            Self::Subagents => "subagents",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowCommunicationContract {
    channel: String,
    inbox: String,
    outbox: String,
    rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowWorkerAssignmentContract {
    workflow_id: String,
    step: String,
    step_kind: String,
    objective: String,
    role: String,
    worker: String,
    result_path: String,
    checks: Vec<String>,
    depends_on: Vec<String>,
    writable_scope: Vec<String>,
    writes_code: bool,
    worktree: Option<String>,
    responsibilities: Vec<String>,
    instructions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowWorkerAssignment {
    worker: String,
    role: String,
    writes: Vec<String>,
    writes_code: Option<bool>,
    worktree: Option<String>,
    responsibilities: Vec<String>,
    checks: Vec<String>,
    contract: WorkflowWorkerAssignmentContract,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowSubagentBatchAssignment {
    step: String,
    step_kind: String,
    contract: WorkflowAgentActionContract,
    input: SubagentInput,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowHeldBackStep {
    step: String,
    step_kind: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkflowNextAction {
    OrchestratedCommandChecks {
        steps: Vec<WorkflowCommandStepRun>,
        reconciled: Vec<String>,
    },
    ValidationBlocked {
        diagnostics: Vec<WorkflowDiagnosticView>,
    },
    AgentAction {
        step: String,
        step_kind: String,
        contract: WorkflowAgentActionContract,
    },
    SubagentAction {
        step: String,
        step_kind: String,
        contract: WorkflowAgentActionContract,
        input: SubagentInput,
    },
    SubagentBatch {
        assignments: Vec<WorkflowSubagentBatchAssignment>,
        held_back: Vec<WorkflowHeldBackStep>,
    },
    MissingActionContract {
        step: String,
        step_kind: String,
        reason: String,
    },
    RunStep {
        step: String,
        step_kind: String,
        worker: Option<String>,
        worker_assignment: Box<Option<WorkflowWorkerAssignment>>,
        checks: Vec<String>,
        workflow: Option<String>,
        depends_on: Vec<String>,
    },
    NoRunnableSteps {
        summary: WorkflowReadinessSummary,
        blocked_steps: Vec<WorkflowBlockedStep>,
    },
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowAgentActionContract {
    workflow_id: String,
    step: String,
    step_kind: String,
    role: String,
    objective: String,
    instructions: Vec<String>,
    write_scope: Vec<String>,
    completion_checks: Vec<String>,
    completion_artifacts: Vec<String>,
    review_required: bool,
    review_rubric: Vec<String>,
    output_required_sections: Vec<String>,
    worker: Option<String>,
    communication: WorkflowCommunicationContract,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowReadinessSummary {
    runnable: usize,
    waiting: usize,
    blocked: usize,
    terminal: usize,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowBlockedStepReason {
    kind: String,
    subject: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowBlockedStep {
    step: String,
    status: String,
    state: String,
    reasons: Vec<String>,
    reason_details: Vec<WorkflowBlockedStepReason>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowUpdateEvent {
    timestamp: String,
    action: String,
    path: String,
    value: serde_json::Value,
    reason: String,
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        "workflow"
    }

    fn label(&self) -> &str {
        "Workflow"
    }

    fn description(&self) -> &str {
        "Inspect, validate, run, and update imp-native workflow artifacts under .imp/workflows. Use list/show/validate/run/update to understand and advance workflow plans."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "show", "validate", "run", "complete_step", "update"],
                    "description": "Workflow action to perform."
                },
                "id": {
                    "type": "string",
                    "description": "Workflow id for show/run/update/validate. Omit for list or to validate all workflows."
                },
                "mode": {
                    "type": "string",
                    "enum": ["strict", "draft"],
                    "description": "Validation mode for validate/show. Defaults to strict."
                },
                "run_mode": {
                    "type": "string",
                    "enum": ["main_agent", "subagents"],
                    "description": "How workflow run should dispatch agent-actionable steps. main_agent returns a contract for the current agent; subagents returns bounded subagent assignments with a communication contract. Defaults to main_agent."
                },
                "step": {
                    "type": "string",
                    "description": "Workflow step id for complete_step."
                },
                "path": {
                    "type": "string",
                    "description": "Workflow object path for update, e.g. steps.verify.status, checks.tests_passed.status, spec.acceptance.done.status, or status."
                },
                "value": {
                    "type": "string",
                    "description": "Replacement status value for update."
                },
                "reason": {
                    "type": "string",
                    "description": "Reason for update, recorded in events.jsonl."
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
        let action = params
            .get("action")
            .and_then(|value| value.as_str())
            .ok_or_else(|| crate::error::Error::Tool("missing `action` parameter".into()))
            .and_then(|value| WorkflowAction::parse(value).map_err(crate::error::Error::Tool))?;
        if !ctx.mode.allows_workflow_action(action.as_str()) {
            let mode_name = format!("{:?}", ctx.mode).to_lowercase();
            return Ok(ToolOutput::error(format!(
                "Workflow action '{}' is not available in {mode_name} mode",
                action.as_str()
            )));
        }

        let id = params
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mode = WorkflowValidationModeParam::parse(params.get("mode").and_then(|v| v.as_str()))
            .map_err(crate::error::Error::Tool)?;
        let run_mode =
            WorkflowExecutionMode::parse(params.get("run_mode").and_then(|v| v.as_str()))
                .map_err(crate::error::Error::Tool)?;
        let workflows_root = workflows_root(&ctx.cwd);

        match action {
            WorkflowAction::List => list_action(&workflows_root),
            WorkflowAction::Show => show_action(&workflows_root, id, mode),
            WorkflowAction::Validate => validate_action(&workflows_root, id, mode),
            WorkflowAction::Run => run_action(&workflows_root, id, mode, run_mode, &ctx).await,
            WorkflowAction::CompleteStep => {
                complete_step_action(&workflows_root, id, &params, &ctx)
            }
            WorkflowAction::Update => update_action(&workflows_root, id, &params, &ctx),
        }
    }
}

#[cfg(test)]
mod tests;
