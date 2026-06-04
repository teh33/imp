use std::path::Path;

use serde_json::json;

use super::checks::{WorkflowCommandStepRun, run_command_checks};
use super::contracts::{
    action_for_runnable_step, subagent_action_for_runnable_step, subagent_batch_for_runnable_steps,
};
use super::files::load_selected_workflow;
use super::readiness::blocked_steps;
use super::render::{CaseExt, render_run_result};
use super::{
    ToolContext, ToolOutput, WorkflowDiagnosticView, WorkflowExecutionMode, WorkflowNextAction,
    WorkflowRunResult, WorkflowValidationModeParam,
};
use crate::error::Result;
use crate::workflow::{StepStatus, load_workflow, next_runnable_steps, validate_workflow};

fn workflow_completed_steps(doc: &crate::workflow::WorkflowDocument) -> usize {
    doc.steps
        .values()
        .filter(|step| matches!(step.status, StepStatus::Done))
        .count()
}

pub(super) async fn run_action(
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

    let (next_action, result_status, result_title, completed_steps, total_steps) = if !diagnostics
        .is_empty()
    {
        (
            WorkflowNextAction::ValidationBlocked {
                diagnostics: diagnostic_views.clone(),
            },
            format!("{:?}", doc.status).to_case(),
            doc.title.clone(),
            workflow_completed_steps(&doc),
            doc.steps.len(),
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
        (
            action,
            result_status,
            current_doc.title.clone(),
            workflow_completed_steps(&current_doc),
            current_doc.steps.len(),
        )
    };

    let result = WorkflowRunResult {
        id: id.clone(),
        title: result_title,
        status: result_status,
        completed_steps,
        total_steps,
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
