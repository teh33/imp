use super::turn_assessment::NextAction;
use super::{LoopDecision, PostTurnAssessment, RunFinalStatus, StopReason};

/// Policy seam for deciding whether a completed turn justifies another turn or
/// should finish with a semantic status.
pub(super) trait LoopPolicy {
    fn decide_after_turn(&self, assessment: &PostTurnAssessment) -> LoopDecision;
}

/// Default loop policy: make the same next-action assessment drive both
/// runtime behavior and debug/trace reporting.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DefaultLoopPolicy;

impl LoopPolicy for DefaultLoopPolicy {
    fn decide_after_turn(&self, assessment: &PostTurnAssessment) -> LoopDecision {
        match assessment.clone().into_next_action() {
            NextAction::Continue { prompt, reason } => LoopDecision::Continue { prompt, reason },
            NextAction::Stop { reason } => finish(reason),
        }
    }
}

fn finish(reason: StopReason) -> LoopDecision {
    LoopDecision::Finish {
        status: RunFinalStatus::from_stop_reason(reason),
    }
}

impl super::Agent {
    pub(super) fn loop_decision_after_turn(&self, assessment: &PostTurnAssessment) -> LoopDecision {
        DefaultLoopPolicy.decide_after_turn(assessment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        ContinueReason, NextActionDebugView, RuntimeEvidence, TextFallbackEvidence,
        WorkflowEvidence,
    };

    fn assessment() -> PostTurnAssessment {
        PostTurnAssessment {
            runtime: RuntimeEvidence {
                repeated_action: false,
                execution_stop_reason: None,
                work_completed: false,
                execution_debt: false,
                execution_evidence: false,
                planning_only_progress: false,
                orchestration_started: false,
            },
            workflow: WorkflowEvidence { stop_reason: None },
            text_fallback: TextFallbackEvidence {
                planner_stop_reason: None,
                execution_stop_reason: None,
            },
            continue_recommendation: None,
        }
    }

    fn final_reason(decision: LoopDecision) -> StopReason {
        match decision {
            LoopDecision::Finish {
                status:
                    RunFinalStatus::Done { reason }
                    | RunFinalStatus::DoneWithConcerns { reason, .. }
                    | RunFinalStatus::Blocked { reason, .. },
            } => reason,
            other => panic!("expected finish decision, got {other:?}"),
        }
    }

    #[test]
    fn repeated_action_wins_over_other_policy_signals() {
        let mut assessment = assessment();
        assessment.runtime.repeated_action = true;
        assessment.runtime.execution_stop_reason = Some(StopReason::ExecutionBlocked);
        assessment.runtime.work_completed = true;
        assessment.continue_recommendation = Some(super::super::ContinueRecommendation {
            prompt: "continue".into(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        });

        assert_eq!(
            final_reason(DefaultLoopPolicy.decide_after_turn(&assessment)),
            StopReason::RepeatedAction
        );
    }

    #[test]
    fn runtime_execution_blocker_wins_over_work_completed() {
        let mut assessment = assessment();
        assessment.runtime.execution_stop_reason = Some(StopReason::ExecutionBlocked);
        assessment.runtime.work_completed = true;

        assert_eq!(
            final_reason(DefaultLoopPolicy.decide_after_turn(&assessment)),
            StopReason::ExecutionBlocked
        );
    }

    #[test]
    fn workflow_stop_wins_over_text_fallback_and_continue() {
        let mut assessment = assessment();
        assessment.workflow.stop_reason = Some(StopReason::UserBlocker);
        assessment.text_fallback.execution_stop_reason = Some(StopReason::WorkCompleted);
        assessment.continue_recommendation = Some(super::super::ContinueRecommendation {
            prompt: "continue".into(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        });

        assert_eq!(
            final_reason(DefaultLoopPolicy.decide_after_turn(&assessment)),
            StopReason::UserBlocker
        );
    }

    #[test]
    fn continue_recommendation_runs_after_stop_reasons_are_absent() {
        let mut assessment = assessment();
        assessment.continue_recommendation = Some(super::super::ContinueRecommendation {
            prompt: "continue".into(),
            reason: ContinueReason::ExecutionDebt,
        });

        assert_eq!(
            DefaultLoopPolicy.decide_after_turn(&assessment),
            LoopDecision::Continue {
                prompt: "continue".into(),
                reason: ContinueReason::ExecutionDebt,
            }
        );
    }

    #[test]
    fn workflow_runner_loop_continues_on_orchestration_progress() {
        let mut assessment = assessment();
        assessment.runtime.orchestration_started = true;
        assessment.runtime.work_completed = true;

        assert_eq!(
            DefaultLoopPolicy.decide_after_turn(&assessment),
            LoopDecision::Continue {
                prompt: crate::workflow::workflow_supervision_prompt(),
                reason: ContinueReason::OrchestrationProgress,
            }
        );
    }

    #[test]
    fn run_the_workflow_continues_from_workflow_run_recommendation() {
        let mut assessment = assessment();
        assessment.continue_recommendation = Some(super::super::ContinueRecommendation {
            prompt: super::super::orchestration_follow_up_text(None),
            reason: ContinueReason::OrchestrationProgress,
        });

        assert_eq!(
            DefaultLoopPolicy.decide_after_turn(&assessment),
            LoopDecision::Continue {
                prompt: crate::workflow::workflow_supervision_prompt(),
                reason: ContinueReason::OrchestrationProgress,
            }
        );
    }

    #[test]
    fn planning_only_progress_maps_to_no_progress_without_continue_reason() {
        let mut assessment = assessment();
        assessment.runtime.planning_only_progress = true;

        assert_eq!(
            final_reason(DefaultLoopPolicy.decide_after_turn(&assessment)),
            StopReason::NoProgress
        );
    }

    #[test]
    fn default_policy_stops_without_automatic_follow_up() {
        assert_eq!(
            final_reason(DefaultLoopPolicy.decide_after_turn(&assessment())),
            StopReason::NoAutomaticFollowUp
        );
    }

    #[test]
    fn default_policy_matches_debug_view_decision() {
        let mut cases = Vec::new();
        cases.push(assessment());

        let mut repeated = assessment();
        repeated.runtime.repeated_action = true;
        cases.push(repeated);

        let mut continue_case = assessment();
        continue_case.continue_recommendation = Some(super::super::ContinueRecommendation {
            prompt: "continue".into(),
            reason: ContinueReason::ExecutionDebt,
        });
        cases.push(continue_case);

        let mut orchestration = assessment();
        orchestration.runtime.orchestration_started = true;
        orchestration.runtime.work_completed = true;
        cases.push(orchestration);

        for assessment in cases {
            let expected = assessment.debug_view().chosen_action;
            let actual = match DefaultLoopPolicy.decide_after_turn(&assessment) {
                LoopDecision::Continue { prompt, reason } => NextActionDebugView::Continue {
                    prompt,
                    reason: reason.as_str().to_string(),
                },
                LoopDecision::Finish { status } => NextActionDebugView::Stop {
                    reason: match status {
                        RunFinalStatus::Done { reason }
                        | RunFinalStatus::DoneWithConcerns { reason, .. }
                        | RunFinalStatus::Blocked { reason, .. } => reason.as_str().to_string(),
                        RunFinalStatus::NeedsUserInput { .. } => {
                            StopReason::UserBlocker.as_str().to_string()
                        }
                        RunFinalStatus::Cancelled | RunFinalStatus::Failed { .. } => {
                            panic!("unexpected terminal status in loop policy consistency test")
                        }
                    },
                },
            };
            assert_eq!(actual, expected);
        }
    }
}
