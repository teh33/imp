use serde::{Deserialize, Serialize};

use crate::workflow::{VerificationCloseoutEffect, VerificationGate, VerificationGateStatus};

/// Coarse-grained phase of a single agent turn.
///
/// This is intentionally small and mechanical: it describes where the runtime
/// is, not why the runtime should continue or stop. Policy decisions live in
/// [`LoopDecision`] / [`RunFinalStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    ReceiveCommands,
    PreTurn,
    BuildContext,
    SampleModel,
    FinalizeAssistantMessage,
    PlanTools,
    ExecuteTools,
    RecordObservations,
    AssessTurn,
    DecideNext,
    Finish,
}

impl TurnPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReceiveCommands => "receive_commands",
            Self::PreTurn => "pre_turn",
            Self::BuildContext => "build_context",
            Self::SampleModel => "sample_model",
            Self::FinalizeAssistantMessage => "finalize_assistant_message",
            Self::PlanTools => "plan_tools",
            Self::ExecuteTools => "execute_tools",
            Self::RecordObservations => "record_observations",
            Self::AssessTurn => "assess_turn",
            Self::DecideNext => "decide_next",
            Self::Finish => "finish",
        }
    }
}

/// Minimal visible state for a turn. This is the first slice of making turn
/// state explicit; later slices can add durable IDs and recovery cursors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnState {
    pub index: u32,
    pub phase: TurnPhase,
    pub continue_reason: Option<ContinueReason>,
    pub planned_tools: usize,
    pub completed_tools: usize,
}

impl TurnState {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            phase: TurnPhase::ReceiveCommands,
            continue_reason: None,
            planned_tools: 0,
            completed_tools: 0,
        }
    }

    pub fn enter(&mut self, phase: TurnPhase) {
        self.phase = phase;
    }

    pub fn record_continue(&mut self, reason: ContinueReason) {
        self.continue_reason = Some(reason);
    }

    pub fn record_tool_plan(&mut self, planned_tools: usize) {
        self.planned_tools = planned_tools;
    }

    pub fn record_tool_results(&mut self, completed_tools: usize) {
        self.completed_tools = completed_tools;
    }
}

/// Why the agent is allowed to spend another turn.
///
/// There should be no anonymous `continue`; long-running autonomy is safe only
/// when each continuation has a concrete, inspectable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueReason {
    ExternalizationNeeded,
    HighConfidenceVisibleNextStep,
    ExecutionDebt,
    CloseoutIncomplete,
    ToolResultsNeedInterpretation,
    QueuedUserFollowUp,
    OrchestrationProgress,
    WorkflowProgress,
    WorkflowCloseout,
    WorkflowBootstrap,
    WorkflowDecomposition,
}

impl ContinueReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExternalizationNeeded => "externalization_needed",
            Self::HighConfidenceVisibleNextStep => "high_confidence_visible_next_step",
            Self::ExecutionDebt => "execution_debt",
            Self::CloseoutIncomplete => "closeout_incomplete",
            Self::ToolResultsNeedInterpretation => "tool_results_need_interpretation",
            Self::QueuedUserFollowUp => "queued_user_follow_up",
            Self::OrchestrationProgress => "orchestration_progress",
            Self::WorkflowProgress => "workflow_progress",
            Self::WorkflowCloseout => "workflow_closeout",
            Self::WorkflowBootstrap => "workflow_bootstrap",
            Self::WorkflowDecomposition => "workflow_decomposition",
        }
    }
}

/// Semantic reason the runtime stopped intentionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    NoAutomaticFollowUp,
    NoProgress,
    RepeatedAction,
    UserBlocker,
    ExecutionBlocked,
    DecompositionCompleted,
    WorkCompleted,
    CloseoutIncomplete,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAutomaticFollowUp => "no_automatic_follow_up",
            Self::NoProgress => "no_progress",
            Self::RepeatedAction => "repeated_action",
            Self::UserBlocker => "user_blocker",
            Self::ExecutionBlocked => "execution_blocked",
            Self::DecompositionCompleted => "decomposition_completed",
            Self::WorkCompleted => "work_completed",
            Self::CloseoutIncomplete => "closeout_incomplete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum LoopDecision {
    Continue {
        reason: ContinueReason,
        prompt: String,
    },
    Finish {
        status: RunFinalStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RunFinalStatus {
    Done {
        reason: StopReason,
    },
    DoneWithConcerns {
        reason: StopReason,
        concerns: Vec<String>,
    },
    Blocked {
        reason: StopReason,
        message: String,
    },
    NeedsUserInput {
        question: String,
    },
    Cancelled,
    Failed {
        message: String,
    },
}

impl RunFinalStatus {
    pub fn from_stop_reason(reason: StopReason) -> Self {
        match reason {
            StopReason::UserBlocker | StopReason::ExecutionBlocked | StopReason::RepeatedAction => {
                Self::Blocked {
                    reason,
                    message: reason.as_str().to_string(),
                }
            }
            StopReason::NoProgress | StopReason::CloseoutIncomplete => Self::DoneWithConcerns {
                reason,
                concerns: vec!["stopped because no justified continuation was available".into()],
            },
            _ => Self::Done { reason },
        }
    }
    pub fn with_concern(self, concern: impl Into<String>) -> Self {
        match self {
            Self::Done { reason } => Self::DoneWithConcerns {
                reason,
                concerns: vec![concern.into()],
            },
            Self::DoneWithConcerns {
                reason,
                mut concerns,
            } => {
                concerns.push(concern.into());
                Self::DoneWithConcerns { reason, concerns }
            }
            other => other,
        }
    }
}

pub fn enforce_verification_closeout(
    proposed: RunFinalStatus,
    gates: &[VerificationGate],
) -> RunFinalStatus {
    if gates.is_empty() {
        return proposed;
    }

    let mut concerns = Vec::new();
    let mut blocked = Vec::new();
    for gate in gates.iter().filter(|gate| gate.is_required()) {
        match gate.closeout_effect() {
            VerificationCloseoutEffect::AllowsDone => {}
            VerificationCloseoutEffect::BlocksDone => blocked.push(verification_gate_message(gate)),
            VerificationCloseoutEffect::BlocksDoneWithConcerns => {
                concerns.push(verification_gate_message(gate));
            }
        }
    }

    if blocked.is_empty() && concerns.is_empty() {
        return proposed;
    }

    if !blocked.is_empty() {
        let message = blocked.join("; ");
        return match proposed {
            RunFinalStatus::Cancelled | RunFinalStatus::Failed { .. } => proposed,
            _ => RunFinalStatus::Blocked {
                reason: StopReason::ExecutionBlocked,
                message,
            },
        };
    }

    match proposed {
        RunFinalStatus::Done { reason } => RunFinalStatus::DoneWithConcerns { reason, concerns },
        RunFinalStatus::DoneWithConcerns {
            reason,
            concerns: mut existing,
        } => {
            existing.extend(concerns);
            RunFinalStatus::DoneWithConcerns {
                reason,
                concerns: existing,
            }
        }
        RunFinalStatus::Blocked { .. }
        | RunFinalStatus::NeedsUserInput { .. }
        | RunFinalStatus::Cancelled
        | RunFinalStatus::Failed { .. } => proposed,
    }
}

#[allow(dead_code)]
pub fn enforce_verification_decision(
    decision: LoopDecision,
    gates: &[VerificationGate],
) -> LoopDecision {
    match decision {
        LoopDecision::Finish { status } => LoopDecision::Finish {
            status: enforce_verification_closeout(status, gates),
        },
        LoopDecision::Continue { .. } => decision,
    }
}

fn verification_gate_message(gate: &VerificationGate) -> String {
    let name = if gate.name.is_empty() {
        &gate.id
    } else {
        &gate.name
    };
    let status = match gate.status {
        VerificationGateStatus::Pending => "pending",
        VerificationGateStatus::Running => "still running",
        VerificationGateStatus::Passed => "passed",
        VerificationGateStatus::Failed => "failed",
        VerificationGateStatus::Skipped => "skipped",
        VerificationGateStatus::Blocked => "blocked",
    };
    let detail = gate.reason.as_deref().or_else(|| {
        gate.result
            .as_ref()
            .and_then(|result| result.summary.as_deref())
    });
    match detail {
        Some(detail) if !detail.is_empty() => {
            format!("required verification {status}: {name} ({detail})")
        }
        _ => format!("required verification {status}: {name}"),
    }
}

/// Tool risk visible before execution. This is deliberately conservative and
/// can be refined as tool metadata grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    Mutable,
    ExternalSideEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    ParallelReadonlyThenSequentialMutable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedToolCall {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
    pub risk: ToolRisk,
    pub retry_safe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPlan {
    pub mode: ToolExecutionMode,
    pub calls: Vec<PlannedToolCall>,
}

impl ToolPlan {
    pub fn empty() -> Self {
        Self {
            mode: ToolExecutionMode::ParallelReadonlyThenSequentialMutable,
            calls: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.calls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[cfg(test)]
mod workflow_closeout_tests {
    use super::*;
    use crate::workflow::{
        VerificationGate, VerificationGateRequirement, VerificationGateResult,
        VerificationGateStatus,
    };

    fn done() -> RunFinalStatus {
        RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        }
    }

    #[test]
    fn workflow_closeout_preserves_done_when_no_gates_or_required_passed() {
        assert_eq!(enforce_verification_closeout(done(), &[]), done());

        let mut gate = VerificationGate::command("unit", "cargo test");
        gate.mark_passed(VerificationGateResult::passed(0));
        assert_eq!(enforce_verification_closeout(done(), &[gate]), done());
    }

    #[test]
    fn workflow_closeout_downgrades_done_for_required_failed_or_skipped_gates() {
        let mut failed = VerificationGate::command("unit", "cargo test");
        failed.mark_failed(VerificationGateResult {
            summary: Some("tests failed".into()),
            ..VerificationGateResult::failed(101)
        });
        let status = enforce_verification_closeout(done(), &[failed]);
        match status {
            RunFinalStatus::DoneWithConcerns { reason, concerns } => {
                assert_eq!(reason, StopReason::WorkCompleted);
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("required verification failed: unit")));
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("tests failed")));
            }
            other => panic!("expected DoneWithConcerns, got {other:?}"),
        }

        let mut skipped = VerificationGate::command("fmt", "cargo fmt --check");
        skipped.mark_skipped("formatter unavailable");
        let status = enforce_verification_closeout(done(), &[skipped]);
        match status {
            RunFinalStatus::DoneWithConcerns { concerns, .. } => {
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("required verification skipped: fmt")));
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("formatter unavailable")));
            }
            other => panic!("expected DoneWithConcerns, got {other:?}"),
        }
    }

    #[test]
    fn workflow_closeout_blocks_done_for_required_blocked_gates() {
        let mut gate = VerificationGate::command("unit", "cargo test");
        gate.mark_blocked("cargo missing");
        let status = enforce_verification_closeout(done(), &[gate]);
        match status {
            RunFinalStatus::Blocked { reason, message } => {
                assert_eq!(reason, StopReason::ExecutionBlocked);
                assert!(message.contains("required verification blocked: unit"));
                assert!(message.contains("cargo missing"));
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn workflow_closeout_pending_and_running_required_gates_cannot_report_done() {
        let pending = VerificationGate::command("unit", "cargo test");
        let status = enforce_verification_closeout(done(), &[pending]);
        assert!(matches!(status, RunFinalStatus::DoneWithConcerns { .. }));

        let mut running = VerificationGate::command("fmt", "cargo fmt --check");
        running.mark_running();
        let status = enforce_verification_closeout(done(), &[running]);
        match status {
            RunFinalStatus::DoneWithConcerns { concerns, .. } => {
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("required verification still running: fmt")));
            }
            other => panic!("expected DoneWithConcerns, got {other:?}"),
        }
    }

    #[test]
    fn workflow_closeout_optional_failed_gate_does_not_block_done() {
        let mut gate = VerificationGate::command("smoke", "cargo test smoke");
        gate.requirement = VerificationGateRequirement::Optional;
        gate.mark_failed(VerificationGateResult::failed(1));
        assert_eq!(enforce_verification_closeout(done(), &[gate]), done());
    }

    #[test]
    fn workflow_closeout_merges_existing_concerns_and_wraps_loop_decision() {
        let mut gate = VerificationGate::command("fmt", "cargo fmt --check");
        gate.status = VerificationGateStatus::Failed;
        let proposed = RunFinalStatus::DoneWithConcerns {
            reason: StopReason::NoProgress,
            concerns: vec!["pre-existing".into()],
        };
        let status = enforce_verification_closeout(proposed, &[gate]);
        match status {
            RunFinalStatus::DoneWithConcerns { reason, concerns } => {
                assert_eq!(reason, StopReason::NoProgress);
                assert!(concerns.iter().any(|concern| concern == "pre-existing"));
                assert!(concerns
                    .iter()
                    .any(|concern| concern.contains("required verification failed: fmt")));
            }
            other => panic!("expected DoneWithConcerns, got {other:?}"),
        }

        let mut blocked = VerificationGate::command("unit", "cargo test");
        blocked.mark_blocked("timeout");
        let decision = LoopDecision::Finish { status: done() };
        assert!(matches!(
            enforce_verification_decision(decision, &[blocked]),
            LoopDecision::Finish {
                status: RunFinalStatus::Blocked { .. }
            }
        ));
    }
}
