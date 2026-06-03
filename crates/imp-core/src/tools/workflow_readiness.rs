use super::workflow_render::CaseExt;
use super::{WorkflowBlockedStep, WorkflowBlockedStepReason, WorkflowReadinessSummary};
use crate::workflow::{
    workflow_step_readiness, WorkflowDocument, WorkflowReadinessReasonKind, WorkflowReadinessState,
};

pub(super) fn blocked_steps(
    doc: &WorkflowDocument,
) -> (WorkflowReadinessSummary, Vec<WorkflowBlockedStep>) {
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
