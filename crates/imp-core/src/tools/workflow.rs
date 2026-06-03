use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::agent::SubagentInput;
use crate::error::Result;
use crate::workflow::{
    load_workflow, load_workflow_raw, next_runnable_steps, validate_workflow,
    workflow_step_readiness, workflow_subagent_input, CheckKind, CheckStatus, ValidateOptions,
    ValidationMode, WorkflowCheck, WorkflowDocument, WorkflowReadinessReasonKind,
    WorkflowReadinessState, WorkflowStep, WorkflowStepAction, WorkflowStepActionKind,
    WorkflowWorker,
};

#[path = "workflow_files.rs"]
mod workflow_files;
#[path = "workflow_render.rs"]
mod workflow_render;
use workflow_files::{
    load_selected_workflow, load_workflow_items, validate_loaded_workflow, workflow_id_root,
    workflow_paths,
};
use workflow_render::{render_run_result, render_workflow, CaseExt};

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

fn workflows_root(cwd: &Path) -> PathBuf {
    cwd.join(".imp").join("workflows")
}

fn list_action(workflows_root: &Path) -> Result<ToolOutput> {
    let workflows = load_workflow_items(workflows_root)?;
    if workflows.is_empty() {
        return Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text {
                text: "No workflows found under .imp/workflows.".to_string(),
            }],
            details: json!({ "action": "list", "workflows": Vec::<WorkflowListItem>::new() }),
            is_error: false,
        });
    }

    let mut text = String::from("Workflows:\n");
    for workflow in &workflows {
        text.push_str(&format!(
            "- {} [{}] {}\n",
            workflow.id, workflow.status, workflow.title
        ));
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({ "action": "list", "workflows": workflows }),
        is_error: false,
    })
}

fn show_action(
    workflows_root: &Path,
    id: Option<&str>,
    mode: WorkflowValidationModeParam,
) -> Result<ToolOutput> {
    let (id, root, doc) = load_selected_workflow(workflows_root, id)?;
    let diagnostics = validate_workflow(&doc, &mode.options(root));
    let text = render_workflow(&id, &doc, &diagnostics);
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "show",
            "id": id,
            "diagnostics": diagnostics.iter().map(|diagnostic| WorkflowDiagnosticView {
                path: diagnostic.path.clone(),
                message: diagnostic.message.clone(),
            }).collect::<Vec<_>>()
        }),
        is_error: false,
    })
}

fn validate_action(
    workflows_root: &Path,
    id: Option<&str>,
    mode: WorkflowValidationModeParam,
) -> Result<ToolOutput> {
    let results = if let Some(id) = id {
        let (_, root, doc) = load_selected_workflow(workflows_root, Some(id))?;
        vec![validate_loaded_workflow(&doc, &root, mode)]
    } else {
        let mut results = Vec::new();
        for path in workflow_paths(workflows_root)? {
            let root = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| workflows_root.to_path_buf());
            let doc = load_workflow(&path).map_err(|error| {
                crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
            })?;
            results.push(validate_loaded_workflow(&doc, &root, mode));
        }
        results
    };

    let ok_count = results.iter().filter(|result| result.ok).count();
    let mut text = format!(
        "Validated {} workflow(s): {} ok, {} with diagnostics.",
        results.len(),
        ok_count,
        results.len().saturating_sub(ok_count)
    );
    for result in &results {
        if result.ok {
            text.push_str(&format!("\n- {}: ok", result.id));
        } else {
            text.push_str(&format!("\n- {}: diagnostics", result.id));
            for diagnostic in &result.diagnostics {
                text.push_str(&format!(
                    "\n  - {}: {}",
                    diagnostic.path, diagnostic.message
                ));
            }
        }
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({ "action": "validate", "results": results }),
        is_error: false,
    })
}

async fn run_action(
    workflows_root: &Path,
    id: Option<&str>,
    mode: WorkflowValidationModeParam,
    run_mode: WorkflowExecutionMode,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let (id, root, doc) = load_selected_workflow(workflows_root, id)?;
    let diagnostics = validate_workflow(&doc, &mode.options(root.clone()));
    let diagnostic_views = diagnostics
        .iter()
        .map(|diagnostic| WorkflowDiagnosticView {
            path: diagnostic.path.clone(),
            message: diagnostic.message.clone(),
        })
        .collect::<Vec<_>>();

    let (next_action, result_status) = if !diagnostics.is_empty() {
        (
            WorkflowNextAction::ValidationBlocked {
                diagnostics: diagnostic_views.clone(),
            },
            format!("{:?}", doc.status).to_case(),
        )
    } else {
        let mut current_doc = doc;
        let mut ran_steps = Vec::new();
        let mut all_reconciled = Vec::new();
        let mut deferred_action = None;

        loop {
            let Some(step_id) = next_runnable_steps(&current_doc).into_iter().next() else {
                break;
            };

            match run_command_checks(workflows_root, &root, &current_doc, &step_id, ctx).await? {
                Some(summary) => {
                    all_reconciled.extend(summary.reconciled.clone());
                    ran_steps.push(WorkflowCommandStepRun {
                        step: step_id,
                        step_status: summary.step_status,
                        checks: summary.checks,
                    });
                    current_doc = load_workflow(&root.join("workflow.yaml")).map_err(|error| {
                        crate::error::Error::Tool(format!(
                            "failed to reload {} after run: {error}",
                            root.join("workflow.yaml").display()
                        ))
                    })?;
                }
                None => {
                    let step = current_doc
                        .steps
                        .get(&step_id)
                        .expect("runnable step exists");
                    if ran_steps.is_empty() {
                        if run_mode == WorkflowExecutionMode::Subagents {
                            if let Some(batch) = subagent_batch_for_runnable_steps(
                                &id,
                                &current_doc,
                                next_runnable_steps(&current_doc),
                                run_mode,
                            ) {
                                deferred_action = Some(batch);
                            } else if let Some(action) = &step.action {
                                deferred_action = Some(subagent_action_for_runnable_step(
                                    &id, &step_id, step, action, run_mode,
                                ));
                            } else {
                                deferred_action = Some(action_for_runnable_step(
                                    &id,
                                    &step_id,
                                    step,
                                    &current_doc,
                                    run_mode,
                                ));
                            }
                        } else {
                            deferred_action = Some(action_for_runnable_step(
                                &id,
                                &step_id,
                                step,
                                &current_doc,
                                run_mode,
                            ));
                        }
                    }
                    break;
                }
            }
        }

        let result_status = format!("{:?}", current_doc.status).to_case();
        let action = if let Some(action) = deferred_action {
            action
        } else if !ran_steps.is_empty() {
            WorkflowNextAction::OrchestratedCommandChecks {
                steps: ran_steps,
                reconciled: all_reconciled,
            }
        } else {
            let (summary, blocked_steps) = blocked_steps(&current_doc);
            WorkflowNextAction::NoRunnableSteps {
                summary,
                blocked_steps,
            }
        };
        (action, result_status)
    };

    let result = WorkflowRunResult {
        id: id.clone(),
        status: result_status,
        execution_mode: run_mode,
        next_action,
    };
    let text = render_run_result(&result);
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({ "action": "run", "id": id, "status": result.status, "result": result }),
        is_error: false,
    })
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowCommandCheckRun {
    check: String,
    command: String,
    status: String,
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowCommandStepRun {
    step: String,
    step_status: String,
    checks: Vec<WorkflowCommandCheckRun>,
}

struct CommandCheckRunSummary {
    checks: Vec<WorkflowCommandCheckRun>,
    step_status: String,
    reconciled: Vec<String>,
}

async fn run_command_checks(
    workflows_root: &Path,
    workflow_root: &Path,
    doc: &WorkflowDocument,
    step_id: &str,
    ctx: &ToolContext,
) -> Result<Option<CommandCheckRunSummary>> {
    let Some(step) = doc.steps.get(step_id) else {
        return Ok(None);
    };
    let runnable = step
        .checks
        .iter()
        .filter_map(|check_id| doc.checks.get(check_id).map(|check| (check_id, check)))
        .filter(|(_, check)| {
            matches!(
                check.kind,
                CheckKind::Command
                    | CheckKind::Presence
                    | CheckKind::Absence
                    | CheckKind::ChangedFiles
            )
        })
        .filter(|(_, check)| matches!(check.status, CheckStatus::Pending))
        .collect::<Vec<_>>();
    if runnable.is_empty() {
        return Ok(None);
    }

    let workflow_path = workflow_root.join("workflow.yaml");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow run denied: {reason}")))?;
    ctx.check_write_path(&event_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow run denied: {reason}")))?;

    let raw = fs::read_to_string(&workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to read {}: {error}",
            workflow_path.display()
        ))
    })?;
    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to parse {}: {error}",
            workflow_path.display()
        ))
    })?;

    let mut event_file = open_workflow_event_file(&event_path)?;
    let mut executed = Vec::new();
    let mut failed_check = false;
    let cwd = workflows_root
        .parent()
        .and_then(Path::parent)
        .unwrap_or(workflows_root);
    for (check_id, check) in runnable {
        let (status, reason, exit_code) = match evaluate_pending_check(check, cwd).await {
            Ok(outcome) => outcome,
            Err(error) => {
                failed_check = true;
                ("failed".to_string(), error, None)
            }
        };
        if status != "passed" {
            failed_check = true;
        }
        set_nested_mapping_string(&mut yaml, &["checks", check_id], "status", &status)?;
        append_workflow_event(
            &mut event_file,
            &WorkflowUpdateEvent {
                timestamp: Utc::now().to_rfc3339(),
                action: "run".to_string(),
                path: format!("checks.{check_id}.status"),
                value: serde_json::Value::String(status.clone()),
                reason,
            },
        )?;
        executed.push(WorkflowCommandCheckRun {
            check: check_id.clone(),
            command: check
                .command
                .clone()
                .unwrap_or_else(|| format!("{:?}", check.kind).to_case()),
            status,
            exit_code,
        });
    }

    let step_status = if failed_check { "failed" } else { "done" };
    set_nested_mapping_string(&mut yaml, &["steps", step_id], "status", step_status)?;
    append_workflow_event(
        &mut event_file,
        &WorkflowUpdateEvent {
            timestamp: Utc::now().to_rfc3339(),
            action: "run".to_string(),
            path: format!("steps.{step_id}.status"),
            value: serde_json::Value::String(step_status.to_string()),
            reason: format!("command checks completed with step status `{step_status}`"),
        },
    )?;

    let reconciled = reconcile_workflow_statuses(&mut yaml, doc, &mut event_file)?;

    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to render workflow yaml: {error}"))
    })?;
    let tmp_path = workflow_path.with_extension("yaml.tmp");
    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {}: {error}",
            workflow_path.display()
        ))
    })?;

    Ok(Some(CommandCheckRunSummary {
        checks: executed,
        step_status: step_status.to_string(),
        reconciled,
    }))
}

fn reconcile_workflow_statuses(
    yaml: &mut serde_yaml::Value,
    doc: &WorkflowDocument,
    event_file: &mut File,
) -> Result<Vec<String>> {
    let mut reconciled = Vec::new();
    let check_passed = |check_id: &str, yaml: &serde_yaml::Value| -> bool {
        yaml.get("checks")
            .and_then(|checks| checks.get(check_id))
            .and_then(|check| check.get("status"))
            .and_then(|status| status.as_str())
            == Some("passed")
    };

    for (acceptance_id, criterion) in &doc.spec.acceptance {
        if !criterion.checks.is_empty()
            && criterion
                .checks
                .iter()
                .all(|check| check_passed(check, yaml))
        {
            let path = format!("spec.acceptance.{acceptance_id}.status");
            set_nested_mapping_string(
                yaml,
                &["spec", "acceptance", acceptance_id],
                "status",
                "done",
            )?;
            append_workflow_event(
                event_file,
                &WorkflowUpdateEvent {
                    timestamp: Utc::now().to_rfc3339(),
                    action: "reconcile".to_string(),
                    path: path.clone(),
                    value: serde_json::Value::String("done".to_string()),
                    reason: "all acceptance checks passed".to_string(),
                },
            )?;
            reconciled.push(path);
        }
    }

    let closeout_ready = doc
        .closeout
        .done
        .requires
        .iter()
        .all(|check| check_passed(check, yaml));
    let all_steps_terminal = yaml
        .get("steps")
        .and_then(|steps| steps.as_mapping())
        .map(|steps| {
            steps.values().all(|step| {
                matches!(
                    step.get("status").and_then(|status| status.as_str()),
                    Some("done" | "done_with_concerns" | "failed" | "blocked" | "skipped")
                )
            })
        })
        .unwrap_or(false);
    if closeout_ready && all_steps_terminal {
        set_mapping_string(yaml, "status", "done")?;
        append_workflow_event(
            event_file,
            &WorkflowUpdateEvent {
                timestamp: Utc::now().to_rfc3339(),
                action: "reconcile".to_string(),
                path: "status".to_string(),
                value: serde_json::Value::String("done".to_string()),
                reason: "closeout requirements passed and all steps are terminal".to_string(),
            },
        )?;
        reconciled.push("status".to_string());
    }

    Ok(reconciled)
}

fn blocked_steps(doc: &WorkflowDocument) -> (WorkflowReadinessSummary, Vec<WorkflowBlockedStep>) {
    let readiness = workflow_step_readiness(doc);
    let summary = WorkflowReadinessSummary {
        runnable: readiness
            .iter()
            .filter(|entry| matches!(entry.state, WorkflowReadinessState::Runnable))
            .count(),
        waiting: readiness
            .iter()
            .filter(|entry| matches!(entry.state, WorkflowReadinessState::Waiting))
            .count(),
        blocked: readiness
            .iter()
            .filter(|entry| matches!(entry.state, WorkflowReadinessState::Blocked))
            .count(),
        terminal: readiness
            .iter()
            .filter(|entry| matches!(entry.state, WorkflowReadinessState::Terminal))
            .count(),
    };

    let blocked_steps = readiness
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.state,
                WorkflowReadinessState::Waiting | WorkflowReadinessState::Blocked
            )
        })
        .map(|entry| {
            let mut reason_details = entry
                .reasons
                .into_iter()
                .map(|reason| WorkflowBlockedStepReason {
                    kind: readiness_reason_kind_label(reason.kind).to_string(),
                    subject: reason.subject,
                    message: reason.message,
                })
                .collect::<Vec<_>>();
            if reason_details.is_empty() {
                reason_details.push(WorkflowBlockedStepReason {
                    kind: "unknown".to_string(),
                    subject: None,
                    message: "waiting for workflow engine support or checks".to_string(),
                });
            }
            WorkflowBlockedStep {
                step: entry.step,
                status: format!("{:?}", entry.status).to_case(),
                state: readiness_state_label(entry.state).to_string(),
                reasons: reason_details
                    .iter()
                    .map(|reason| reason.message.clone())
                    .collect(),
                reason_details,
            }
        })
        .collect();

    (summary, blocked_steps)
}

fn readiness_state_label(state: WorkflowReadinessState) -> &'static str {
    match state {
        WorkflowReadinessState::Runnable => "runnable",
        WorkflowReadinessState::Waiting => "waiting",
        WorkflowReadinessState::Blocked => "blocked",
        WorkflowReadinessState::Terminal => "terminal",
    }
}

fn readiness_reason_kind_label(kind: WorkflowReadinessReasonKind) -> &'static str {
    match kind {
        WorkflowReadinessReasonKind::DependencyMissing => "dependency_missing",
        WorkflowReadinessReasonKind::DependencyNotReady => "dependency_not_ready",
        WorkflowReadinessReasonKind::WorkerMissing => "worker_missing",
        WorkflowReadinessReasonKind::StatusNotRunnable => "status_not_runnable",
        WorkflowReadinessReasonKind::CheckPending => "check_pending",
        WorkflowReadinessReasonKind::CheckFailed => "check_failed",
        WorkflowReadinessReasonKind::CheckBlocked => "check_blocked",
    }
}

async fn evaluate_pending_check(
    check: &WorkflowCheck,
    cwd: &Path,
) -> std::result::Result<(String, String, Option<i32>), String> {
    match check.kind {
        CheckKind::Command => {
            let command = check
                .command
                .as_deref()
                .ok_or_else(|| "command check is missing command".to_string())?;
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output()
                .await
                .map_err(|error| format!("failed to run command check: {error}"))?;
            let status = if output.status.success() {
                "passed"
            } else {
                "failed"
            };
            Ok((
                status.to_string(),
                format!(
                    "command `{}` exited with {}",
                    command,
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string())
                ),
                output.status.code(),
            ))
        }
        CheckKind::Presence | CheckKind::Absence => {
            let path = check
                .path
                .as_ref()
                .or(check.file.as_ref())
                .ok_or_else(|| "presence/absence check is missing path or file".to_string())?;
            let pattern = check
                .pattern
                .as_deref()
                .ok_or_else(|| "presence/absence check is missing pattern".to_string())?;
            let full_path = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let content = fs::read_to_string(&full_path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let contains = content.contains(pattern);
            let passed = match check.kind {
                CheckKind::Presence => contains,
                CheckKind::Absence => !contains,
                _ => unreachable!(),
            };
            let status = if passed { "passed" } else { "failed" };
            let expectation = if matches!(check.kind, CheckKind::Presence) {
                "contains"
            } else {
                "does not contain"
            };
            Ok((
                status.to_string(),
                format!("{} {} `{}`", path.display(), expectation, pattern),
                None,
            ))
        }
        CheckKind::ChangedFiles => {
            if check.paths.is_empty() {
                return Err("changed_files check is missing paths".to_string());
            }
            let mut command = tokio::process::Command::new("git");
            command
                .arg("status")
                .arg("--porcelain")
                .arg("--")
                .args(&check.paths)
                .current_dir(cwd);
            let output = command
                .output()
                .await
                .map_err(|error| format!("failed to inspect git status: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "git status failed with {}",
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string())
                ));
            }
            let changed = !String::from_utf8_lossy(&output.stdout).trim().is_empty();
            let status = if changed { "passed" } else { "failed" };
            Ok((
                status.to_string(),
                format!(
                    "changed files check inspected {} path(s)",
                    check.paths.len()
                ),
                output.status.code(),
            ))
        }
        _ => Err(format!("check kind {:?} is not runnable", check.kind)),
    }
}

const MAX_SUBAGENT_BATCH_ASSIGNMENTS: usize = 4;

fn subagent_batch_for_runnable_steps(
    workflow_id: &str,
    doc: &WorkflowDocument,
    runnable_steps: Vec<String>,
    run_mode: WorkflowExecutionMode,
) -> Option<WorkflowNextAction> {
    let mut assignments = Vec::new();
    let mut held_back = Vec::new();
    let mut selected_scopes: Vec<(String, Vec<PathBuf>)> = Vec::new();

    for step_id in runnable_steps {
        let Some(step) = doc.steps.get(&step_id) else {
            continue;
        };
        let step_kind = format!("{:?}", step.kind).to_case();
        let Some(action) = &step.action else {
            held_back.push(WorkflowHeldBackStep {
                step: step_id,
                step_kind,
                reason: "missing explicit action contract".to_string(),
            });
            continue;
        };

        if assignments.len() >= MAX_SUBAGENT_BATCH_ASSIGNMENTS {
            held_back.push(WorkflowHeldBackStep {
                step: step_id,
                step_kind,
                reason: format!("subagent batch cap of {MAX_SUBAGENT_BATCH_ASSIGNMENTS} reached"),
            });
            continue;
        }

        if action.write_scope.is_empty() {
            held_back.push(WorkflowHeldBackStep {
                step: step_id,
                step_kind,
                reason: "missing write scope for safe parallel dispatch".to_string(),
            });
            continue;
        }

        if let Some((conflicting_step, _)) = selected_scopes
            .iter()
            .find(|(_, scopes)| write_scopes_overlap(scopes, &action.write_scope))
        {
            held_back.push(WorkflowHeldBackStep {
                step: step_id,
                step_kind,
                reason: format!("write scope overlaps with {conflicting_step}"),
            });
            continue;
        }

        selected_scopes.push((step_id.clone(), action.write_scope.clone()));
        assignments.push(WorkflowSubagentBatchAssignment {
            step: step_id.clone(),
            step_kind: step_kind.clone(),
            contract: agent_action_contract(workflow_id, &step_id, &step_kind, action, run_mode),
            input: workflow_subagent_input(workflow_id, &step_id, action),
        });
    }

    if assignments.len() > 1 {
        Some(WorkflowNextAction::SubagentBatch {
            assignments,
            held_back,
        })
    } else {
        None
    }
}

fn write_scopes_overlap(left: &[PathBuf], right: &[PathBuf]) -> bool {
    left.iter()
        .any(|left| right.iter().any(|right| write_scope_overlaps(left, right)))
}

fn write_scope_overlaps(left: &Path, right: &Path) -> bool {
    let left = normalize_write_scope(left);
    let right = normalize_write_scope(right);

    if is_broad_write_scope(&left) || is_broad_write_scope(&right) {
        return true;
    }
    if left == right {
        return true;
    }

    let left_prefix = left.strip_suffix("/**").unwrap_or(&left);
    let right_prefix = right.strip_suffix("/**").unwrap_or(&right);
    left_prefix == right_prefix
        || left_prefix.starts_with(&format!("{right_prefix}/"))
        || right_prefix.starts_with(&format!("{left_prefix}/"))
}

fn normalize_write_scope(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            std::path::Component::CurDir => None,
            _ => Some("**"),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_broad_write_scope(scope: &str) -> bool {
    matches!(scope, "" | "." | "**") || scope.contains("*") && !scope.ends_with("/**")
}

fn subagent_action_for_runnable_step(
    workflow_id: &str,
    step_id: &str,
    step: &WorkflowStep,
    action: &WorkflowStepAction,
    run_mode: WorkflowExecutionMode,
) -> WorkflowNextAction {
    let step_kind = format!("{:?}", step.kind).to_case();
    WorkflowNextAction::SubagentAction {
        step: step_id.to_string(),
        step_kind: step_kind.clone(),
        contract: agent_action_contract(workflow_id, step_id, &step_kind, action, run_mode),
        input: workflow_subagent_input(workflow_id, step_id, action),
    }
}

fn action_for_runnable_step(
    workflow_id: &str,
    step_id: &str,
    step: &WorkflowStep,
    doc: &WorkflowDocument,
    run_mode: WorkflowExecutionMode,
) -> WorkflowNextAction {
    let step_kind = format!("{:?}", step.kind).to_case();

    if let Some(action) = &step.action {
        return WorkflowNextAction::AgentAction {
            step: step_id.to_string(),
            step_kind: step_kind.clone(),
            contract: agent_action_contract(workflow_id, step_id, &step_kind, action, run_mode),
        };
    }

    if step.workflow.is_some() || step.worker.is_some() {
        let worker_assignment = step.worker.as_ref().and_then(|worker_id| {
            doc.workers.get(worker_id).map(|worker| {
                worker_assignment(
                    workflow_id,
                    step_id,
                    step,
                    worker_id,
                    worker,
                    &step.checks,
                    doc,
                )
            })
        });
        return WorkflowNextAction::RunStep {
            step: step_id.to_string(),
            step_kind,
            worker: step.worker.clone(),
            worker_assignment: Box::new(worker_assignment),
            checks: step.checks.clone(),
            workflow: step.workflow.clone(),
            depends_on: step.depends_on.clone(),
        };
    }

    WorkflowNextAction::MissingActionContract {
        step: step_id.to_string(),
        step_kind,
        reason: "Add command checks, a child workflow, worker, or action contract.".to_string(),
    }
}

fn agent_action_contract(
    workflow_id: &str,
    step_id: &str,
    step_kind: &str,
    action: &WorkflowStepAction,
    run_mode: WorkflowExecutionMode,
) -> WorkflowAgentActionContract {
    let role = action.role.clone().unwrap_or_else(|| match action.kind {
        WorkflowStepActionKind::Agent => "coder".to_string(),
        WorkflowStepActionKind::Worker => "worker".to_string(),
    });
    WorkflowAgentActionContract {
        workflow_id: workflow_id.to_string(),
        step: step_id.to_string(),
        step_kind: step_kind.to_string(),
        role,
        objective: action.objective.clone(),
        instructions: {
            let mut instructions = action.instructions.clone();
            instructions.push(format!(
                "When this step is complete, call workflow(action=\"complete_step\", id=\"{workflow_id}\", step=\"{step_id}\", reason=\"...\") to mark the step and its checks complete."
            ));
            instructions
        },
        write_scope: action
            .write_scope
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        completion_checks: action.completion.checks.clone(),
        completion_artifacts: action
            .completion
            .artifacts
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        worker: action.worker.clone(),
        communication: workflow_communication_contract(workflow_id, step_id, run_mode),
    }
}

fn workflow_communication_contract(
    workflow_id: &str,
    step_id: &str,
    run_mode: WorkflowExecutionMode,
) -> WorkflowCommunicationContract {
    let base = format!(".imp/workflows/{workflow_id}/artifacts/communication/{step_id}");
    let rules = match run_mode {
        WorkflowExecutionMode::MainAgent => vec![
            "The main agent owns this step and should update workflow status/checks when complete.".to_string(),
            "If help is needed, record questions or blockers in the outbox before pausing.".to_string(),
        ],
        WorkflowExecutionMode::Subagents => vec![
            "Subagents write progress, questions, blockers, and final outcomes to the outbox.".to_string(),
            "The main agent watches the inbox/outbox, answers questions, merges outcomes, and updates workflow status/checks.".to_string(),
            "Subagents must not write outside their declared write scope without main-agent approval.".to_string(),
        ],
    };
    WorkflowCommunicationContract {
        channel: match run_mode {
            WorkflowExecutionMode::MainAgent => "main_agent_artifact_mailbox".to_string(),
            WorkflowExecutionMode::Subagents => "subagent_artifact_mailbox".to_string(),
        },
        inbox: format!("{base}/inbox.md"),
        outbox: format!("{base}/outbox.md"),
        rules,
    }
}

fn worker_assignment(
    workflow_id: &str,
    step_id: &str,
    step: &WorkflowStep,
    worker_id: &str,
    worker: &WorkflowWorker,
    checks: &[String],
    doc: &WorkflowDocument,
) -> WorkflowWorkerAssignment {
    let step_kind = format!("{:?}", step.kind).to_case();
    let writes_code = worker.writes_code.unwrap_or_else(|| {
        worker
            .writes
            .iter()
            .any(|scope| scope == "code" || scope == "tests")
    });
    let role = workflow_worker_role(worker_id, worker);
    let objective = workflow_worker_objective(step_id, &step_kind, doc);
    let result_path = doc.results.path.display().to_string();
    let instructions = workflow_worker_instructions(
        workflow_id,
        step_id,
        &step_kind,
        &role,
        &result_path,
        checks,
        writes_code,
    );
    let contract = WorkflowWorkerAssignmentContract {
        workflow_id: workflow_id.to_string(),
        step: step_id.to_string(),
        step_kind,
        objective,
        role: role.clone(),
        worker: worker_id.to_string(),
        result_path,
        checks: checks.to_vec(),
        depends_on: step.depends_on.clone(),
        writable_scope: worker.writes.clone(),
        writes_code,
        worktree: worker.worktree.clone(),
        responsibilities: worker.responsibilities.clone(),
        instructions,
    };
    WorkflowWorkerAssignment {
        worker: worker_id.to_string(),
        role: worker.role.clone(),
        writes: worker.writes.clone(),
        writes_code: worker.writes_code,
        worktree: worker.worktree.clone(),
        responsibilities: worker.responsibilities.clone(),
        checks: checks.to_vec(),
        contract,
    }
}

fn workflow_worker_role(worker_id: &str, worker: &WorkflowWorker) -> String {
    match worker.role.as_str() {
        "builder" => "coder".to_string(),
        "review" => "reviewer".to_string(),
        "verify" => "verifier".to_string(),
        role if role.trim().is_empty() => worker_id.to_string(),
        role => role.to_string(),
    }
}

fn workflow_worker_objective(step_id: &str, step_kind: &str, doc: &WorkflowDocument) -> String {
    format!(
        "Complete workflow step `{step_id}` ({step_kind}) for `{}`: {}",
        doc.id,
        doc.spec.goal.trim()
    )
}

fn workflow_worker_instructions(
    workflow_id: &str,
    step_id: &str,
    step_kind: &str,
    role: &str,
    result_path: &str,
    checks: &[String],
    writes_code: bool,
) -> Vec<String> {
    let mut instructions = vec![
        format!("You are the `{role}` worker for workflow `{workflow_id}`."),
        format!("Work only on step `{step_id}` ({step_kind}) and do not broaden scope."),
        format!("Record outcome, verification, concerns, and next steps in `{result_path}`."),
    ];
    if checks.is_empty() {
        instructions.push(
            "No explicit workflow checks are attached; state the verification you performed."
                .to_string(),
        );
    } else {
        instructions.push(format!(
            "Satisfy or honestly block these workflow checks: {}.",
            checks.join(", ")
        ));
    }
    if writes_code {
        instructions.push("Make focused code/test changes only within the declared writable scope and run the narrowest relevant verification.".to_string());
    } else {
        instructions.push(
            "Do not modify production code unless the parent explicitly grants write scope."
                .to_string(),
        );
    }
    instructions
}

fn complete_step_action(
    workflows_root: &Path,
    id: Option<&str>,
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let id = id.ok_or_else(|| crate::error::Error::Tool("complete_step requires `id`".into()))?;
    let step_id = params
        .get("step")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("complete_step requires `step`".into()))?;
    let reason = params
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("complete_step requires `reason`".into()))?;

    let workflow_root = workflow_id_root(workflows_root, id)?;
    let workflow_path = workflow_root.join("workflow.yaml");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;
    ctx.check_write_path(&event_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;

    let raw = load_workflow_raw(&workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to read {}: {error}",
            workflow_path.display()
        ))
    })?;
    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to parse {}: {error}",
            workflow_path.display()
        ))
    })?;
    let doc: WorkflowDocument = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to load workflow document {}: {error}",
            workflow_path.display()
        ))
    })?;
    let step = doc
        .steps
        .get(step_id)
        .ok_or_else(|| crate::error::Error::Tool(format!("unknown workflow step `{step_id}`")))?;

    let mut event_file = open_workflow_event_file(&event_path)?;
    set_nested_mapping_string(&mut yaml, &["steps", step_id], "status", "done")?;
    append_workflow_event(
        &mut event_file,
        &WorkflowUpdateEvent {
            timestamp: Utc::now().to_rfc3339(),
            action: "complete_step".to_string(),
            path: format!("steps.{step_id}.status"),
            value: serde_json::Value::String("done".to_string()),
            reason: reason.to_string(),
        },
    )?;

    let mut completed_checks = Vec::new();
    for check_id in &step.checks {
        if doc.checks.contains_key(check_id) {
            set_nested_mapping_string(&mut yaml, &["checks", check_id], "status", "passed")?;
            append_workflow_event(
                &mut event_file,
                &WorkflowUpdateEvent {
                    timestamp: Utc::now().to_rfc3339(),
                    action: "complete_step".to_string(),
                    path: format!("checks.{check_id}.status"),
                    value: serde_json::Value::String("passed".to_string()),
                    reason: format!("step `{step_id}` completed: {reason}"),
                },
            )?;
            completed_checks.push(check_id.clone());
        }
    }

    let reconciled = reconcile_workflow_statuses(&mut yaml, &doc, &mut event_file)?;
    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize workflow: {error}"))
    })?;
    let candidate: WorkflowDocument = serde_yaml::from_str(&updated).map_err(|error| {
        crate::error::Error::Tool(format!(
            "workflow complete_step would produce invalid YAML/schema: {error}"
        ))
    })?;
    let diagnostics =
        validate_workflow(&candidate, &ValidateOptions::strict(workflow_root.clone()));
    if !diagnostics.is_empty() {
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(crate::error::Error::Tool(format!(
            "workflow complete_step failed validation: {rendered}"
        )));
    }

    let tmp_path = workflow_path.with_extension("yaml.tmp");
    ctx.check_write_path(&tmp_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;
    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {} with {}: {error}",
            workflow_path.display(),
            tmp_path.display()
        ))
    })?;

    let text = format!("Completed workflow `{id}` step `{step_id}`.");
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "complete_step",
            "id": id,
            "step": step_id,
            "checks": completed_checks,
            "reconciled": reconciled,
            "reason": reason,
        }),
        is_error: false,
    })
}

fn update_action(
    workflows_root: &Path,
    id: Option<&str>,
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let id = id.ok_or_else(|| crate::error::Error::Tool("update requires `id`".into()))?;
    let update_path = params
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires `path`".into()))?;
    let value = params
        .get("value")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires string `value`".into()))?;
    let reason = params
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires `reason`".into()))?;

    let workflow_root = workflow_id_root(workflows_root, id)?;
    let workflow_path = workflow_root.join("workflow.yaml");
    let raw = load_workflow_raw(&workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to read {}: {error}",
            workflow_path.display()
        ))
    })?;
    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to parse {}: {error}",
            workflow_path.display()
        ))
    })?;

    apply_status_update(&mut yaml, update_path, value)?;

    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize workflow: {error}"))
    })?;
    let candidate: WorkflowDocument = serde_yaml::from_str(&updated).map_err(|error| {
        crate::error::Error::Tool(format!(
            "workflow update would produce invalid YAML/schema: {error}"
        ))
    })?;
    let diagnostics =
        validate_workflow(&candidate, &ValidateOptions::strict(workflow_root.clone()));
    if !diagnostics.is_empty() {
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(crate::error::Error::Tool(format!(
            "workflow update failed validation: {rendered}"
        )));
    }

    let tmp_path = workflow_path.with_extension("yaml.tmp");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;
    ctx.check_write_path(&tmp_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;
    ctx.check_write_path(&event_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;

    let event = WorkflowUpdateEvent {
        timestamp: Utc::now().to_rfc3339(),
        action: "update".to_string(),
        path: update_path.to_string(),
        value: serde_json::Value::String(value.to_string()),
        reason: reason.to_string(),
    };
    let mut event_file = open_workflow_event_file(&event_path)?;

    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {} with {}: {error}",
            workflow_path.display(),
            tmp_path.display()
        ))
    })?;

    append_workflow_event(&mut event_file, &event)?;

    let text = format!("Updated workflow `{id}`: {update_path} = {value}");
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "id": id,
            "path": update_path,
            "value": value,
            "reason": reason
        }),
        is_error: false,
    })
}

fn apply_status_update(yaml: &mut serde_yaml::Value, update_path: &str, value: &str) -> Result<()> {
    let parts = update_path.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["status"] => set_mapping_string(yaml, "status", value),
        ["steps", id, "status"] => set_nested_mapping_string(yaml, &["steps", id], "status", value),
        ["checks", id, "status"] => {
            set_nested_mapping_string(yaml, &["checks", id], "status", value)
        }
        ["prototypes", id, "status"] => {
            set_nested_mapping_string(yaml, &["prototypes", id], "status", value)
        }
        ["spec", "acceptance", id, "status"] => {
            set_nested_mapping_string(yaml, &["spec", "acceptance", id], "status", value)
        }
        _ => Err(crate::error::Error::Tool(format!(
            "unsupported workflow update path `{update_path}`"
        ))),
    }
}

fn set_nested_mapping_string(
    yaml: &mut serde_yaml::Value,
    path: &[&str],
    key: &str,
    value: &str,
) -> Result<()> {
    let mut current = yaml;
    for segment in path {
        current = mapping_get_mut(current, segment).ok_or_else(|| {
            crate::error::Error::Tool(format!("workflow path segment `{segment}` not found"))
        })?;
    }
    set_mapping_string(current, key, value)
}

fn set_mapping_string(yaml: &mut serde_yaml::Value, key: &str, value: &str) -> Result<()> {
    let mapping = yaml.as_mapping_mut().ok_or_else(|| {
        crate::error::Error::Tool(format!("workflow path target for `{key}` is not a map"))
    })?;
    let key_value = serde_yaml::Value::String(key.to_string());
    if !mapping.contains_key(&key_value) {
        return Err(crate::error::Error::Tool(format!(
            "workflow path key `{key}` not found"
        )));
    }
    mapping.insert(key_value, serde_yaml::Value::String(value.to_string()));
    Ok(())
}

fn mapping_get_mut<'a>(
    yaml: &'a mut serde_yaml::Value,
    key: &str,
) -> Option<&'a mut serde_yaml::Value> {
    yaml.as_mapping_mut()?
        .get_mut(serde_yaml::Value::String(key.to_string()))
}

fn open_workflow_event_file(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(Into::into)
}

fn append_workflow_event(file: &mut std::fs::File, event: &WorkflowUpdateEvent) -> Result<()> {
    serde_json::to_writer(&mut *file, event).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize event: {error}"))
    })?;
    writeln!(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::StepStatus;
    use std::sync::Arc;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn workflow_tool_list_discovers_workflows() {
        let output = list_action(&repo_root().join(".imp/workflows")).expect("list succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("prototype-imp-workflow-engine"));
        assert!(text.contains("prototype-workflow-tool"));
    }

    #[test]
    fn workflow_tool_show_renders_status() {
        let output = show_action(
            &repo_root().join(".imp/workflows"),
            Some("update-imp-after-workflow-engine"),
            WorkflowValidationModeParam::Strict,
        )
        .expect("show succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("Workflow: update-imp-after-workflow-engine"));
        assert!(text.contains("Acceptance:"));
        assert!(text.contains("Steps:"));
    }

    #[test]
    fn workflow_tool_validate_all_passes_for_dogfood_workflows() {
        let output = validate_action(
            &repo_root().join(".imp/workflows"),
            None,
            WorkflowValidationModeParam::Strict,
        )
        .expect("validate succeeds");
        assert!(!output.is_error);
        let text = output.text_content().expect("text output");
        assert!(text.contains("Validated"));
        assert!(text.contains("0 with diagnostics"), "{text}");
    }

    #[tokio::test]
    async fn workflow_run_returns_next_runnable_step() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
        set_step_status(
            &workflows_root,
            "implement-workflow-run-engine",
            "execute",
            "todo",
        );

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("implement-workflow-run-engine"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Next workflow action: run step execute [build]"),
            "{text}"
        );
        assert!(text.contains("Worker: builder"), "{text}");
        assert!(
            text.contains("Worker assignment: builder (builder)"),
            "{text}"
        );
        assert!(text.contains("Writes: code, tests"), "{text}");
        assert!(text.contains("Worktree: workflow"), "{text}");

        let assignment = output.details["result"]["next_action"]["worker_assignment"]
            .as_object()
            .expect("worker assignment details");
        let contract = assignment["contract"]
            .as_object()
            .expect("worker assignment contract");
        assert_eq!(contract["workflow_id"], "implement-workflow-run-engine");
        assert_eq!(contract["step"], "execute");
        assert_eq!(contract["step_kind"], "build");
        assert_eq!(contract["role"], "coder");
        assert_eq!(contract["worker"], "builder");
        assert_eq!(
            contract["result_path"],
            ".imp/workflows/implement-workflow-run-engine/results.md"
        );
        assert_eq!(contract["writes_code"], true);
        assert_eq!(contract["worktree"], "workflow");
        assert!(contract["objective"]
            .as_str()
            .expect("objective")
            .contains("Complete workflow step `execute`"));
        let instructions = contract["instructions"].as_array().expect("instructions");
        assert!(instructions.iter().any(|instruction| {
            instruction
                .as_str()
                .is_some_and(|text| text.contains("do not broaden scope"))
        }));
        assert!(instructions.iter().any(|instruction| {
            instruction
                .as_str()
                .is_some_and(|text| text.contains("implementation_ready"))
        }));
    }

    #[tokio::test]
    async fn workflow_run_executes_pending_command_checks() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let (workflows_root, workflow_root) = write_command_check_workflow(temp.path(), true);

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("command-check-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("ran 1 command check"), "{text}");
        assert!(text.contains("command_check: passed"), "{text}");

        let doc = load_workflow(&workflow_root.join("workflow.yaml"))
            .expect("updated workflow should load");
        assert!(matches!(
            doc.steps.get("verify").expect("step exists").status,
            StepStatus::Done
        ));
        assert!(matches!(doc.status, crate::workflow::WorkflowStatus::Done));
        assert!(matches!(
            doc.spec
                .acceptance
                .get("command_check_passes")
                .expect("acceptance exists")
                .status,
            crate::workflow::AcceptanceStatus::Done
        ));
        let events = std::fs::read_to_string(workflow_root.join("events.jsonl"))
            .expect("events should be written");
        assert!(events.contains("checks.command_check.status"), "{events}");
        assert!(
            events.contains("spec.acceptance.command_check_passes.status"),
            "{events}"
        );
        assert!(events.contains("\"path\":\"status\""), "{events}");
    }

    #[tokio::test]
    async fn workflow_run_executes_presence_absence_checks() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_presence_absence_workflow(temp.path());
        std::fs::write(temp.path().join("subject.txt"), "alpha\nbeta\n").expect("write subject");

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("presence-absence-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("has_alpha: passed"), "{text}");
        assert!(text.contains("no_gamma: passed"), "{text}");

        let doc = load_workflow(&workflows_root.join("presence-absence-workflow/workflow.yaml"))
            .expect("updated workflow loads");
        assert!(matches!(
            doc.checks.get("has_alpha").expect("check exists").status,
            CheckStatus::Passed
        ));
        assert!(matches!(
            doc.checks.get("no_gamma").expect("check exists").status,
            CheckStatus::Passed
        ));
        assert!(matches!(
            doc.steps.get("verify").expect("step exists").status,
            StepStatus::Done
        ));
    }

    #[tokio::test]
    async fn workflow_run_executes_changed_files_check() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        init_git_repo(temp.path());
        std::fs::write(temp.path().join("tracked.txt"), "before\n").expect("write tracked");
        git(temp.path(), &["add", "tracked.txt"]);
        git(temp.path(), &["commit", "-m", "initial"]);
        std::fs::write(temp.path().join("tracked.txt"), "after\n").expect("modify tracked");
        let workflows_root = write_changed_files_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("changed-files-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("source_changed: passed"), "{text}");

        let doc = load_workflow(&workflows_root.join("changed-files-workflow/workflow.yaml"))
            .expect("updated workflow loads");
        assert!(matches!(
            doc.checks
                .get("source_changed")
                .expect("check exists")
                .status,
            CheckStatus::Passed
        ));
        assert!(matches!(
            doc.steps.get("verify").expect("step exists").status,
            StepStatus::Done
        ));
    }

    #[tokio::test]
    async fn workflow_run_does_not_complete_build_from_broad_checks_only() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_broad_only_build_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("broad-only-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run returns validation diagnostics");
        let text = output.text_content().expect("text output");
        assert!(text.contains("blocked by validation diagnostics"), "{text}");
        assert!(text.contains("broad command checks alone"), "{text}");

        let doc = load_workflow(&workflows_root.join("broad-only-workflow/workflow.yaml"))
            .expect("workflow still loads");
        assert!(matches!(
            doc.steps.get("implement").expect("step exists").status,
            StepStatus::Ready
        ));
        assert!(matches!(
            doc.checks.get("broad_tests").expect("check exists").status,
            CheckStatus::Pending
        ));
    }

    #[tokio::test]
    async fn workflow_run_marks_step_failed_when_command_check_fails() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let (workflows_root, workflow_root) = write_command_check_workflow(temp.path(), false);

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("command-check-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("command_check: failed"), "{text}");
        assert!(text.contains("- step verify: failed"), "{text}");

        let doc = load_workflow(&workflow_root.join("workflow.yaml"))
            .expect("updated workflow should load");
        assert!(matches!(
            doc.checks
                .get("command_check")
                .expect("check exists")
                .status,
            CheckStatus::Failed
        ));
        assert!(matches!(
            doc.steps.get("verify").expect("step exists").status,
            StepStatus::Failed
        ));
    }

    #[tokio::test]
    async fn workflow_run_reports_no_runnable_steps_when_dependencies_block() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
        set_step_status(
            &workflows_root,
            "implement-workflow-run-engine",
            "verify",
            "todo",
        );

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("implement-workflow-run-engine"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow blocked: missing action contract for step verify [verify]."),
            "{text}"
        );
    }

    #[tokio::test]
    async fn workflow_run_reports_readiness_summary_when_no_steps_are_runnable() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_no_runnable_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("no-runnable-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(text.contains("No runnable workflow steps."), "{text}");
        assert!(text.contains("Readiness summary:"), "{text}");
        assert!(text.contains("- waiting: 2"), "{text}");
        assert!(text.contains("- terminal: 0"), "{text}");
        assert!(text.contains("step status is active"), "{text}");
        assert!(text.contains("dependency `inspect` is active"), "{text}");

        let next_action = &output.details["result"]["next_action"];
        assert_eq!(next_action["kind"], "no_runnable_steps");
        assert_eq!(next_action["summary"]["waiting"], 2);
        assert_eq!(next_action["summary"]["terminal"], 0);
        assert_eq!(
            next_action["blocked_steps"][1]["reason_details"][0]["kind"],
            "dependency_not_ready"
        );
    }

    #[tokio::test]
    async fn workflow_complete_step_marks_step_checks_and_workflow_done() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_agent_action_workflow(temp.path());
        let ctx = test_ctx(temp.path());

        let output = complete_step_action(
            &workflows_root,
            Some("agent-action-workflow"),
            &json!({
                "step": "inspect",
                "reason": "inspection artifact written"
            }),
            &ctx,
        )
        .expect("complete step succeeds");

        assert_eq!(output.details["action"], "complete_step");
        assert_eq!(output.details["step"], "inspect");
        let doc = load_workflow(&workflows_root.join("agent-action-workflow/workflow.yaml"))
            .expect("updated workflow loads");
        assert!(matches!(
            doc.steps.get("inspect").expect("step exists").status,
            StepStatus::Done
        ));
        assert!(matches!(
            doc.checks.get("inspected").expect("check exists").status,
            CheckStatus::Passed
        ));
        assert!(matches!(doc.status, crate::workflow::WorkflowStatus::Done));
        assert!(matches!(
            doc.spec
                .acceptance
                .get("inspected")
                .expect("acceptance exists")
                .status,
            crate::workflow::AcceptanceStatus::Done
        ));
    }

    #[tokio::test]
    async fn workflow_run_renders_agent_action_contract() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_agent_action_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("agent-action-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow needs main agent action: inspect [context]"),
            "{text}"
        );
        assert!(text.contains("Role: coder"), "{text}");
        assert!(
            text.contains("Objective: Inspect workflow action support."),
            "{text}"
        );
        assert!(text.contains("Instructions:"), "{text}");
        assert!(text.contains("Allowed writes:"), "{text}");
        assert!(text.contains("Completion:"), "{text}");
        assert!(text.contains("complete_step"), "{text}");
        assert!(text.contains("Communication:"), "{text}");
        assert!(text.contains("main_agent_artifact_mailbox"), "{text}");
        assert!(
            text.contains("artifacts/communication/inspect/inbox.md"),
            "{text}"
        );
        assert!(text.contains("- check: inspected"), "{text}");
        assert_eq!(
            output.details["result"]["next_action"]["kind"],
            "agent_action"
        );
        assert_eq!(output.details["action"], "run");
        assert_eq!(output.details["id"], "agent-action-workflow");
        assert_eq!(output.details["status"], "active");
    }

    #[tokio::test]
    async fn workflow_run_renders_subagent_action_contract() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_agent_action_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("agent-action-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::Subagents,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow recommends subagent action: inspect [context]"),
            "{text}"
        );
        assert!(text.contains("subagent_artifact_mailbox"), "{text}");
        assert!(
            text.contains("workflow-agent-action-workflow-inspect"),
            "{text}"
        );
        assert!(
            text.contains("Launch this with the Subagent tool"),
            "{text}"
        );
        assert_eq!(output.details["result"]["execution_mode"], "subagents");
        assert_eq!(
            output.details["result"]["next_action"]["kind"],
            "subagent_action"
        );
        assert_eq!(
            output.details["result"]["next_action"]["contract"]["communication"]["channel"],
            "subagent_artifact_mailbox"
        );
        assert_eq!(
            output.details["result"]["next_action"]["input"]["child_run_id"],
            "workflow-agent-action-workflow-inspect"
        );
    }

    #[tokio::test]
    async fn workflow_run_renders_subagent_batch_for_parallel_action_steps() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_parallel_action_workflow(temp.path(), false);

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("parallel-action-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::Subagents,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow recommends 3 parallel subagent action(s)."),
            "{text}"
        );
        assert!(text.contains("- cli [build]: CLI coder"), "{text}");
        assert!(text.contains("- core [build]: Core coder"), "{text}");
        assert_eq!(
            output.details["result"]["next_action"]["kind"],
            "subagent_batch"
        );
        assert_eq!(
            output.details["result"]["next_action"]["assignments"]
                .as_array()
                .expect("assignments")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn workflow_run_holds_back_overlapping_subagent_write_scope() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_parallel_action_workflow(temp.path(), true);

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("parallel-action-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::Subagents,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow recommends 2 parallel subagent action(s)."),
            "{text}"
        );
        assert!(text.contains("Held back:"), "{text}");
        assert!(text.contains("write scope overlaps with cli"), "{text}");
        assert_eq!(
            output.details["result"]["next_action"]["assignments"]
                .as_array()
                .expect("assignments")
                .len(),
            2
        );
        assert_eq!(
            output.details["result"]["next_action"]["held_back"]
                .as_array()
                .expect("held back")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn workflow_run_blocks_missing_action_contract() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_missing_action_contract_workflow(temp.path());

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("missing-action-workflow"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run succeeds");
        let text = output.text_content().expect("text output");
        assert!(
            text.contains("Workflow blocked: missing action contract for step inspect [context]."),
            "{text}"
        );
        assert!(
            text.contains("Add command checks, a child workflow, worker, or action contract."),
            "{text}"
        );
        assert_eq!(
            output.details["result"]["next_action"]["kind"],
            "missing_action_contract"
        );
    }

    #[test]
    fn workflow_validation_rejects_bad_action_contract() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_invalid_action_workflow(temp.path());
        let (_, root, doc) =
            load_selected_workflow(&workflows_root, Some("invalid-action-workflow"))
                .expect("workflow loads");
        let diagnostics = validate_workflow(&doc, &ValidateOptions::strict(root));
        let messages = diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            messages.contains("steps.inspect.action.objective: action objective must not be empty"),
            "{messages}"
        );
        assert!(
            messages.contains("steps.inspect.action.worker: unknown worker `missing_worker`"),
            "{messages}"
        );
        assert!(
            messages
                .contains("steps.inspect.action.completion.checks: unknown check `missing_check`"),
            "{messages}"
        );
    }

    #[tokio::test]
    async fn workflow_run_reports_validation_diagnostics() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);
        let workflow_path = workflows_root
            .join("implement-workflow-run-engine")
            .join("workflow.yaml");
        let raw = std::fs::read_to_string(&workflow_path)
            .expect("fixture copied")
            .replace(
                "checks:\n      - implementation_ready",
                "checks:\n      - missing_check",
            );
        std::fs::write(&workflow_path, raw).expect("write broken fixture");

        let ctx = test_ctx(temp.path());
        let output = run_action(
            &workflows_root,
            Some("implement-workflow-run-engine"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        .expect("run returns diagnostics instead of error");
        let text = output.text_content().expect("text output");
        assert!(text.contains("blocked by validation diagnostics"), "{text}");
        assert!(text.contains("unknown check `missing_check`"), "{text}");
    }

    #[test]
    fn workflow_update_status_updates_yaml_and_appends_event() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-update-events", &workflows_root);

        let ctx = test_ctx(temp.path());
        let output = update_action(
            &workflows_root,
            Some("implement-workflow-update-events"),
            &json!({
                "path": "steps.execute.status",
                "value": "done",
                "reason": "unit test completed execute step"
            }),
            &ctx,
        )
        .expect("update succeeds");
        assert!(output
            .text_content()
            .expect("text output")
            .contains("Updated workflow"));

        let workflow_path = workflows_root
            .join("implement-workflow-update-events")
            .join("workflow.yaml");
        let doc = load_workflow(&workflow_path).expect("updated workflow should load");
        assert!(matches!(
            doc.steps.get("execute").expect("step exists").status,
            crate::workflow::StepStatus::Done
        ));

        let events = std::fs::read_to_string(
            workflows_root
                .join("implement-workflow-update-events")
                .join("events.jsonl"),
        )
        .expect("events should be written");
        assert!(events.contains("steps.execute.status"));
        assert!(events.contains("unit test completed execute step"));
    }

    #[tokio::test]
    async fn workflow_tool_execute_enforces_mode_action_policy() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-run-engine", &workflows_root);

        let mut ctx = test_ctx(temp.path());
        ctx.mode = crate::config::AgentMode::Auditor;
        let output = WorkflowTool
            .execute(
                "test-call",
                json!({
                    "action": "update",
                    "id": "implement-workflow-run-engine",
                    "path": "steps.execute.status",
                    "value": "done",
                    "reason": "unit test should be blocked"
                }),
                ctx,
            )
            .await
            .expect("policy denial returns tool output");

        assert!(output.is_error);
        let text = output.text_content().expect("text output");
        assert!(text.contains("not available in auditor mode"), "{text}");
        assert!(!workflows_root
            .join("implement-workflow-run-engine")
            .join("events.jsonl")
            .exists());
    }

    #[test]
    fn workflow_update_rejects_invalid_status_without_writing() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
        let workflow_path = workflows_root
            .join("implement-workflow-update-events")
            .join("workflow.yaml");
        let before = std::fs::read_to_string(&workflow_path).expect("fixture copied");

        let ctx = test_ctx(temp.path());
        let error = match update_action(
            &workflows_root,
            Some("implement-workflow-update-events"),
            &json!({
                "path": "steps.execute.status",
                "value": "not_a_status",
                "reason": "unit test invalid status"
            }),
            &ctx,
        ) {
            Ok(_) => panic!("invalid status should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("invalid YAML/schema"));
        let after = std::fs::read_to_string(&workflow_path).expect("fixture remains");
        assert_eq!(before, after);
        assert!(!workflows_root
            .join("implement-workflow-update-events")
            .join("events.jsonl")
            .exists());
    }

    #[tokio::test]
    async fn workflow_rejects_absolute_or_parent_directory_ids() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
        let ctx = test_ctx(temp.path());

        let show_error = match show_action(
            &workflows_root,
            Some("../implement-workflow-update-events"),
            WorkflowValidationModeParam::Strict,
        ) {
            Ok(_) => panic!("parent traversal id should fail"),
            Err(error) => error,
        };
        assert!(show_error.to_string().contains("invalid workflow id"));

        let run_error = match run_action(
            &workflows_root,
            Some("/tmp/implement-workflow-update-events"),
            WorkflowValidationModeParam::Strict,
            WorkflowExecutionMode::MainAgent,
            &ctx,
        )
        .await
        {
            Ok(_) => panic!("absolute id should fail"),
            Err(error) => error,
        };
        assert!(run_error.to_string().contains("invalid workflow id"));

        let nested_error = match show_action(
            &workflows_root,
            Some("nested/implement-workflow-update-events"),
            WorkflowValidationModeParam::Strict,
        ) {
            Ok(_) => panic!("nested id should fail"),
            Err(error) => error,
        };
        assert!(nested_error.to_string().contains("invalid workflow id"));

        let update_error = match update_action(
            &workflows_root,
            Some("../implement-workflow-update-events"),
            &json!({
                "path": "steps.execute.status",
                "value": "done",
                "reason": "unit test invalid id"
            }),
            &ctx,
        ) {
            Ok(_) => panic!("update traversal id should fail"),
            Err(error) => error,
        };
        assert!(update_error.to_string().contains("invalid workflow id"));
    }

    #[test]
    fn workflow_update_rejects_unwritable_event_log_without_replacing_yaml() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = temp.path().join(".imp/workflows");
        copy_workflow_fixture("implement-workflow-update-events", &workflows_root);
        let workflow_root = workflows_root.join("implement-workflow-update-events");
        let workflow_path = workflow_root.join("workflow.yaml");
        let before = std::fs::read_to_string(&workflow_path).expect("fixture copied");
        std::fs::create_dir(workflow_root.join("events.jsonl"))
            .expect("create conflicting event log directory");

        let ctx = test_ctx(temp.path());
        let error = match update_action(
            &workflows_root,
            Some("implement-workflow-update-events"),
            &json!({
                "path": "steps.execute.status",
                "value": "done",
                "reason": "unit test event log failure"
            }),
            &ctx,
        ) {
            Ok(_) => panic!("unwritable event log should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("Is a directory"), "{error}");
        let after = std::fs::read_to_string(&workflow_path).expect("fixture remains");
        assert_eq!(before, after);
        assert!(!workflow_path.with_extension("yaml.tmp").exists());
    }

    fn set_step_status(workflows_root: &Path, workflow_id: &str, step_id: &str, status: &str) {
        let workflow_path = workflows_root.join(workflow_id).join("workflow.yaml");
        let raw = std::fs::read_to_string(&workflow_path).expect("fixture copied");
        let marker = format!("  {step_id}:\n");
        let start = raw.find(&marker).expect("step marker exists");
        let rest = &raw[start..];
        let status_marker = "    status: ";
        let status_start =
            start + rest.find(status_marker).expect("step status exists") + status_marker.len();
        let status_end = raw[status_start..]
            .find('\n')
            .map(|offset| status_start + offset)
            .expect("status line ends");
        let mut updated = raw;
        updated.replace_range(status_start..status_end, status);
        std::fs::write(&workflow_path, updated).expect("write fixture");
    }

    fn test_ctx(dir: &Path) -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
        ToolContext {
            cwd: dir.to_path_buf(),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx: tx,
            command_tx: cmd_tx,
            ui: Arc::new(crate::ui::NullInterface),
            file_cache: Arc::new(crate::tools::FileCache::new()),
            checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
            anchor_store: Arc::new(crate::tools::AnchorStore::new()),
            lua_tool_loader: None,
            mode: crate::config::AgentMode::Full,
            read_max_lines: 500,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: Arc::new(crate::config::Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        }
    }

    fn write_command_check_workflow(root: &Path, command_succeeds: bool) -> (PathBuf, PathBuf) {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("command-check-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        let command = if command_succeeds { "true" } else { "false" };
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            format!(
                r#"schema: imp.workflow/v1
id: command-check-workflow
title: Command check workflow
status: active
kind: implementation
settings:
  worktree: none
  strictness: medium
  durable: true
  disposable: false
  commit_traces: false
spec:
  goal: Run command checks.
  acceptance:
    command_check_passes:
      text: Command check passes.
      status: todo
      checks:
        - command_check
steps:
  verify:
    kind: verify
    status: ready
    checks:
      - command_check
checks:
  command_check:
    kind: command
    status: pending
    command: {command}
results:
  path: .imp/workflows/command-check-workflow/results.md
workers: {{}}
closeout:
  done:
    requires:
      - command_check
"#
            ),
        )
        .expect("write workflow");
        (workflows_root, workflow_root)
    }

    fn write_presence_absence_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("presence-absence-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: presence-absence-workflow
title: Presence absence workflow
status: active
kind: implementation
spec:
  goal: Run presence and absence checks.
  acceptance:
    content_checked:
      text: Content is checked.
      status: todo
      checks: [has_alpha, no_gamma]
steps:
  verify:
    kind: verify
    status: ready
    checks: [has_alpha, no_gamma]
checks:
  has_alpha:
    kind: presence
    status: pending
    path: subject.txt
    pattern: alpha
  no_gamma:
    kind: absence
    status: pending
    path: subject.txt
    pattern: gamma
results:
  path: .imp/workflows/presence-absence-workflow/results.md
workers: {}
closeout:
  done:
    requires: [has_alpha, no_gamma]
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_changed_files_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("changed-files-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: changed-files-workflow
title: Changed files workflow
status: active
kind: implementation
spec:
  goal: Run changed files check.
  acceptance:
    source_changed:
      text: Source changed.
      status: todo
      checks: [source_changed]
steps:
  verify:
    kind: verify
    status: ready
    checks: [source_changed]
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [tracked.txt]
results:
  path: .imp/workflows/changed-files-workflow/results.md
workers: {}
closeout:
  done:
    requires: [source_changed]
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_broad_only_build_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("broad-only-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: broad-only-workflow
title: Broad only workflow
status: active
kind: implementation
spec:
  goal: Broad command checks alone should not complete implementation.
  acceptance:
    done:
      text: Work is done.
      status: todo
      checks: [broad_tests]
steps:
  implement:
    kind: build
    status: ready
    checks: [broad_tests]
checks:
  broad_tests:
    kind: command
    status: pending
    broad: true
    command: true
results:
  path: .imp/workflows/broad-only-workflow/results.md
workers: {}
closeout:
  done:
    requires: [broad_tests]
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn init_git_repo(root: &Path) {
        git(root, &["init"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test User"]);
    }

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_no_runnable_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("no-runnable-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: no-runnable-workflow
title: No runnable workflow
status: active
kind: test
spec:
  goal: Report readiness when blocked.
  acceptance:
    done:
      text: Readiness is reported.
      status: todo
steps:
  inspect:
    kind: context
    status: active
  verify:
    kind: verify
    status: todo
    depends_on:
      - inspect
checks: {}
results:
  path: .imp/workflows/no-runnable-workflow/results.md
workers: {}
closeout:
  done:
    requires:
      - no_unapproved_goal_or_acceptance_changes
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_agent_action_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("agent-action-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: agent-action-workflow
title: Agent action workflow
status: active
kind: implementation
spec:
  goal: Dispatch agent action.
  acceptance:
    inspected:
      text: Agent action is dispatched.
      status: todo
      checks:
        - inspected
steps:
  inspect:
    kind: context
    status: ready
    checks:
      - inspected
    action:
      kind: agent
      role: coder
      objective: Inspect workflow action support.
      instructions:
        - Read the workflow runner.
        - Report the action contract.
      write_scope:
        - .imp/workflows/agent-action-workflow/artifacts/inspection.md
      completion:
        checks:
          - inspected
        artifacts:
          - .imp/workflows/agent-action-workflow/artifacts/inspection.md
checks:
  inspected:
    kind: review
    status: pending
results:
  path: .imp/workflows/agent-action-workflow/results.md
workers: {}
closeout:
  done:
    requires:
      - inspected
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_parallel_action_workflow(root: &Path, overlap: bool) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("parallel-action-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        let docs_scope = if overlap {
            "crates/imp-cli/src/lib.rs"
        } else {
            "docs/workflows.md"
        };
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            format!(
                r#"schema: imp.workflow/v1
id: parallel-action-workflow
title: Parallel action workflow
status: active
kind: test
spec:
  goal: Dispatch parallel action steps.
  acceptance:
    done:
      text: Parallel actions are dispatched.
      status: todo
      checks:
        - cli_done
        - core_done
        - docs_done
steps:
  cli:
    kind: build
    status: ready
    checks:
      - cli_done
    action:
      kind: agent
      role: CLI coder
      objective: Update CLI workflow behavior.
      write_scope:
        - crates/imp-cli/src/lib.rs
      completion:
        checks:
          - cli_done
  core:
    kind: build
    status: ready
    checks:
      - core_done
    action:
      kind: agent
      role: Core coder
      objective: Update core workflow behavior.
      write_scope:
        - crates/imp-core/src/tools/workflow.rs
      completion:
        checks:
          - core_done
  docs:
    kind: build
    status: ready
    checks:
      - docs_done
    action:
      kind: agent
      role: Docs writer
      objective: Update workflow docs.
      write_scope:
        - {docs_scope}
      completion:
        checks:
          - docs_done
checks:
  cli_done:
    kind: review
    status: pending
  core_done:
    kind: review
    status: pending
  docs_done:
    kind: review
    status: pending
results:
  path: .imp/workflows/parallel-action-workflow/results.md
workers: {{}}
closeout:
  done:
    requires:
      - cli_done
      - core_done
      - docs_done
"#
            ),
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_missing_action_contract_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("missing-action-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: missing-action-workflow
title: Missing action workflow
status: active
kind: implementation
spec:
  goal: Report missing action contract.
  acceptance:
    inspected:
      text: Missing action is reported.
      status: todo
      checks:
        - inspected
steps:
  inspect:
    kind: context
    status: ready
    checks:
      - inspected
checks:
  inspected:
    kind: review
    status: pending
results:
  path: .imp/workflows/missing-action-workflow/results.md
workers: {}
closeout:
  done:
    requires:
      - inspected
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn write_invalid_action_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("invalid-action-workflow");
        std::fs::create_dir_all(&workflow_root).expect("create workflow root");
        std::fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: invalid-action-workflow
title: Invalid action workflow
status: active
kind: implementation
spec:
  goal: Validate action contracts.
  acceptance:
    inspected:
      text: Invalid action is rejected.
      status: todo
      checks:
        - inspected
steps:
  inspect:
    kind: context
    status: ready
    action:
      kind: worker
      worker: missing_worker
      objective: ""
      completion:
        checks:
          - missing_check
checks:
  inspected:
    kind: review
    status: pending
results:
  path: .imp/workflows/invalid-action-workflow/results.md
workers: {}
closeout:
  done:
    requires:
      - inspected
"#,
        )
        .expect("write workflow");
        workflows_root
    }

    fn copy_workflow_fixture(id: &str, workflows_root: &Path) {
        let source = repo_root()
            .join(".imp/workflows")
            .join(id)
            .join("workflow.yaml");
        let destination_dir = workflows_root.join(id);
        std::fs::create_dir_all(&destination_dir).expect("create fixture workflow dir");
        std::fs::copy(source, destination_dir.join("workflow.yaml"))
            .expect("copy fixture workflow");

        if id == "implement-workflow-update-events" || id == "implement-workflow-run-engine" {
            copy_workflow_fixture("prototype-imp-workflow-engine", workflows_root);
            for relative_path in ["artifacts/plan.md", "results.md"] {
                let path = destination_dir.join(relative_path);
                std::fs::create_dir_all(path.parent().expect("fixture artifact parent"))
                    .expect("create fixture artifact dir");
                std::fs::write(path, "fixture artifact").expect("write fixture artifact");
            }

            let project_root = workflows_root
                .parent()
                .and_then(Path::parent)
                .expect("workflow root should be under .imp/workflows");
            let source_artifact = project_root.join("crates/imp-core/src/tools/workflow.rs");
            std::fs::create_dir_all(source_artifact.parent().expect("source artifact parent"))
                .expect("create source artifact dir");
            std::fs::write(source_artifact, "fixture source artifact")
                .expect("write source artifact");
        }
    }
}
