use super::{WorkflowNextAction, WorkflowRunResult};
use crate::workflow::{CheckStatus, StepKind, WorkflowDocument};

pub(super) fn render_run_result(result: &WorkflowRunResult) -> String {
    match &result.next_action {
        WorkflowNextAction::OrchestratedCommandChecks { steps, reconciled } => {
            let check_count: usize = steps.iter().map(|step| step.checks.len()).sum();
            let mut lines = vec![format!(
                "Workflow `{}` orchestrated {} step(s) and ran {} command check(s).",
                result.id,
                steps.len(),
                check_count
            )];
            for step in steps {
                lines.push(format!("- step {}: {}", step.step, step.step_status));
                for check in &step.checks {
                    lines.push(format!(
                        "  - {}: {} (exit {})",
                        check.check,
                        check.status,
                        check
                            .exit_code
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "signal".to_string())
                    ));
                }
            }
            if !reconciled.is_empty() {
                lines.push(format!("Reconciled: {}", reconciled.join(", ")));
            }
            lines.join("\n")
        }
        WorkflowNextAction::AgentAction {
            step,
            step_kind,
            contract,
        } => {
            let mut lines = vec![format!(
                "Workflow needs {} action: {step} [{step_kind}]",
                result.execution_mode.label()
            )];
            lines.push(String::new());
            lines.push(format!("Role: {}", contract.role));
            if let Some(worker) = &contract.worker {
                lines.push(format!("Worker: {worker}"));
            }
            lines.push(format!("Objective: {}", contract.objective));
            if !contract.instructions.is_empty() {
                lines.push(String::new());
                lines.push("Instructions:".to_string());
                for instruction in &contract.instructions {
                    lines.push(format!("- {instruction}"));
                }
            }
            if !contract.write_scope.is_empty() {
                lines.push(String::new());
                lines.push("Allowed writes:".to_string());
                for path in &contract.write_scope {
                    lines.push(format!("- {path}"));
                }
            }
            if !contract.completion_checks.is_empty() || !contract.completion_artifacts.is_empty() {
                lines.push(String::new());
                lines.push("Completion:".to_string());
                for check in &contract.completion_checks {
                    lines.push(format!("- check: {check}"));
                }
                for artifact in &contract.completion_artifacts {
                    lines.push(format!("- artifact: {artifact}"));
                }
            }
            lines.push(String::new());
            lines.push("Communication:".to_string());
            lines.push(format!("- channel: {}", contract.communication.channel));
            lines.push(format!("- inbox: {}", contract.communication.inbox));
            lines.push(format!("- outbox: {}", contract.communication.outbox));
            for rule in &contract.communication.rules {
                lines.push(format!("- rule: {rule}"));
            }
            lines.join("\n")
        }
        WorkflowNextAction::SubagentAction {
            step,
            step_kind,
            contract,
            input,
        } => {
            let mut lines = vec![format!(
                "Workflow recommends subagent action: {step} [{step_kind}] ({})",
                input.child_run_id.as_str()
            )];
            lines.push(String::new());
            lines.push(format!("Role: {}", contract.role));
            lines.push(format!("Objective: {}", contract.objective));
            lines.push(String::new());
            lines.push("Communication:".to_string());
            lines.push(format!("- channel: {}", contract.communication.channel));
            lines.push(format!("- inbox: {}", contract.communication.inbox));
            lines.push(format!("- outbox: {}", contract.communication.outbox));
            lines.push(String::new());
            lines.push("Launch this with the Subagent tool; workflow state is unchanged until the subagent reports an outcome.".to_string());
            lines.join("\n")
        }
        WorkflowNextAction::SubagentBatch {
            assignments,
            held_back,
        } => {
            let mut lines = vec![format!(
                "Workflow recommends {} parallel subagent action(s).",
                assignments.len()
            )];
            if !assignments.is_empty() {
                lines.push(String::new());
                lines.push("Launchable:".to_string());
                for assignment in assignments {
                    lines.push(format!(
                        "- {} [{}]: {}",
                        assignment.step, assignment.step_kind, assignment.contract.role
                    ));
                    lines.push(format!("  - objective: {}", assignment.contract.objective));
                    if !assignment.contract.write_scope.is_empty() {
                        lines.push(format!(
                            "  - writes: {}",
                            assignment.contract.write_scope.join(", ")
                        ));
                    }
                }
            }
            if !held_back.is_empty() {
                lines.push(String::new());
                lines.push("Held back:".to_string());
                for step in held_back {
                    lines.push(format!(
                        "- {} [{}]: {}",
                        step.step, step.step_kind, step.reason
                    ));
                }
            }
            lines.push(String::new());
            lines.push("Launch each assignment with the Subagent tool; workflow state is unchanged until subagents report outcomes.".to_string());
            lines.join("\n")
        }
        WorkflowNextAction::MissingActionContract {
            step,
            step_kind,
            reason,
        } => format!(
            "Workflow blocked: missing action contract for step {step} [{step_kind}].\n{reason}"
        ),
        WorkflowNextAction::ValidationBlocked { diagnostics } => {
            let mut text = format!(
                "Workflow `{}` is blocked by validation diagnostics:",
                result.id
            );
            for diagnostic in diagnostics {
                text.push_str(&format!("\n- {}: {}", diagnostic.path, diagnostic.message));
            }
            text
        }
        WorkflowNextAction::RunStep {
            step,
            step_kind,
            worker,
            worker_assignment,
            checks,
            workflow,
            depends_on,
        } => {
            let mut text = format!("Next workflow action: run step {step} [{step_kind}]");
            if let Some(worker) = worker {
                text.push_str(&format!("\nWorker: {worker}"));
            }
            if let Some(assignment) = worker_assignment.as_ref().as_ref() {
                text.push_str(&format!(
                    "\nWorker assignment: {} ({})",
                    assignment.worker, assignment.role
                ));
                if !assignment.writes.is_empty() {
                    text.push_str(&format!("\nWrites: {}", assignment.writes.join(", ")));
                }
                if let Some(worktree) = assignment.worktree.as_ref() {
                    text.push_str(&format!("\nWorktree: {worktree}"));
                }
            }
            if let Some(workflow) = workflow {
                text.push_str(&format!("\nWorkflow: {workflow}"));
            }
            if !depends_on.is_empty() {
                text.push_str(&format!("\nDepends on: {}", depends_on.join(", ")));
            }
            if !checks.is_empty() {
                text.push_str(&format!("\nChecks: {}", checks.join(", ")));
            }
            text
        }
        WorkflowNextAction::NoRunnableSteps {
            summary,
            blocked_steps,
        } => {
            let mut text = String::from("No runnable workflow steps.");
            text.push_str("\n\nReadiness summary:");
            text.push_str(&format!("\n- runnable: {}", summary.runnable));
            text.push_str(&format!("\n- waiting: {}", summary.waiting));
            text.push_str(&format!("\n- blocked: {}", summary.blocked));
            text.push_str(&format!("\n- terminal: {}", summary.terminal));
            if !blocked_steps.is_empty() {
                text.push_str("\n\nBlocked/pending steps:");
                for step in blocked_steps {
                    text.push_str(&format!("\n- {} [{}]", step.step, step.status));
                    for reason in &step.reasons {
                        text.push_str(&format!("\n  - {reason}"));
                    }
                }
            }
            text
        }
    }
}

pub(super) fn render_workflow(
    id: &str,
    doc: &WorkflowDocument,
    diagnostics: &[crate::workflow::WorkflowDiagnostic],
) -> String {
    let acceptance_done = doc
        .spec
        .acceptance
        .values()
        .filter(|criterion| matches!(criterion.status, crate::workflow::AcceptanceStatus::Done))
        .count();
    let mut text = format!(
        "Workflow: {} [{}]\nTitle: {}\nGoal: {}\nAcceptance: {}/{} done\nResults: {}\n",
        id,
        format!("{:?}", doc.status).to_case(),
        doc.title,
        doc.spec.goal.trim(),
        acceptance_done,
        doc.spec.acceptance.len(),
        doc.results.path.display()
    );

    if let Some(parent) = &doc.parent {
        text.push_str(&format!("Parent: {}#{}\n", parent.workflow, parent.step));
    }

    let child_workflows = doc
        .steps
        .iter()
        .filter_map(|(id, step)| match (&step.kind, &step.workflow) {
            (StepKind::Workflow, Some(workflow)) => Some(format!("{id}->{workflow}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !child_workflows.is_empty() {
        text.push_str(&format!(
            "Child workflows: {}\n",
            child_workflows.join(", ")
        ));
    }

    text.push_str("Steps:\n");
    for (step_id, step) in &doc.steps {
        text.push_str(&format!(
            "- {} [{}] {}\n",
            step_id,
            format!("{:?}", step.status).to_case(),
            format!("{:?}", step.kind).to_case()
        ));
    }

    let pending_checks = doc
        .checks
        .iter()
        .filter(|(_, check)| !matches!(check.status, CheckStatus::Passed))
        .collect::<Vec<_>>();
    if !pending_checks.is_empty() {
        text.push_str("Checks needing attention:\n");
        for (check_id, check) in pending_checks {
            text.push_str(&format!(
                "- {} [{}] {}\n",
                check_id,
                format!("{:?}", check.status).to_case(),
                format!("{:?}", check.kind).to_case()
            ));
        }
    }

    if !diagnostics.is_empty() {
        text.push_str("Diagnostics:\n");
        for diagnostic in diagnostics {
            text.push_str(&format!("- {}: {}\n", diagnostic.path, diagnostic.message));
        }
    }

    text
}

pub(super) trait CaseExt {
    fn to_case(&self) -> String;
}

impl CaseExt for str {
    fn to_case(&self) -> String {
        let mut out = String::new();
        for (index, ch) in self.chars().enumerate() {
            if ch.is_uppercase() && index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        }
        out
    }
}
