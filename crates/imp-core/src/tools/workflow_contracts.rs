use std::path::{Path, PathBuf};

use super::workflow_render::CaseExt;
use super::{
    WorkflowAgentActionContract, WorkflowCommunicationContract, WorkflowExecutionMode,
    WorkflowHeldBackStep, WorkflowNextAction, WorkflowSubagentBatchAssignment,
    WorkflowWorkerAssignment, WorkflowWorkerAssignmentContract,
};
use crate::workflow::{
    workflow_subagent_input, WorkflowDocument, WorkflowStep, WorkflowStepAction,
    WorkflowStepActionKind, WorkflowWorker,
};

const MAX_SUBAGENT_BATCH_ASSIGNMENTS: usize = 4;

pub(super) fn subagent_batch_for_runnable_steps(
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

pub(super) fn subagent_action_for_runnable_step(
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

pub(super) fn action_for_runnable_step(
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
