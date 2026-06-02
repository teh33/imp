use super::{LoopDecision, PostTurnAssessment, RunFinalStatus, StopReason};

/// Policy seam for deciding whether a completed turn justifies another turn or
/// should finish with a semantic status.
pub(super) trait LoopPolicy {
    fn decide_after_turn(&self, assessment: &PostTurnAssessment) -> LoopDecision;
}

trait LoopPolicyRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision>;
}

/// Default compatibility policy: preserve the existing post-turn assessment
/// ordering while moving each decision behind a replaceable rule.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DefaultLoopPolicy;

impl LoopPolicy for DefaultLoopPolicy {
    fn decide_after_turn(&self, assessment: &PostTurnAssessment) -> LoopDecision {
        RepeatedActionRule
            .decide(assessment)
            .or_else(|| RuntimeStopRule.decide(assessment))
            .or_else(|| WorkCompletedRule.decide(assessment))
            .or_else(|| OrchestrationProgressRule.decide(assessment))
            .or_else(|| WorkflowStopRule.decide(assessment))
            .or_else(|| TextFallbackStopRule.decide(assessment))
            .or_else(|| ContinueRecommendationRule.decide(assessment))
            .or_else(|| PlanningOnlyNoProgressRule.decide(assessment))
            .unwrap_or_else(|| finish(StopReason::NoAutomaticFollowUp))
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RepeatedActionRule;

impl LoopPolicyRule for RepeatedActionRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment
            .runtime
            .repeated_action
            .then(|| finish(StopReason::RepeatedAction))
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RuntimeStopRule;

impl LoopPolicyRule for RuntimeStopRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment.runtime.execution_stop_reason.map(finish)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct WorkCompletedRule;

impl LoopPolicyRule for WorkCompletedRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        if assessment.runtime.work_completed && !assessment.runtime.orchestration_started {
            Some(finish(StopReason::WorkCompleted))
        } else {
            None
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct OrchestrationProgressRule;

impl LoopPolicyRule for OrchestrationProgressRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment
            .runtime
            .orchestration_started
            .then(|| LoopDecision::Continue {
                prompt: super::orchestration_follow_up_text(None),
                reason: super::ContinueReason::OrchestrationProgress,
            })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct WorkflowStopRule;

impl LoopPolicyRule for WorkflowStopRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment.workflow.stop_reason.map(finish)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct TextFallbackStopRule;

impl LoopPolicyRule for TextFallbackStopRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment
            .text_fallback
            .planner_stop_reason
            .or(assessment.text_fallback.execution_stop_reason)
            .map(finish)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ContinueRecommendationRule;

impl LoopPolicyRule for ContinueRecommendationRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment
            .continue_recommendation
            .as_ref()
            .map(|recommendation| LoopDecision::Continue {
                prompt: recommendation.prompt.clone(),
                reason: recommendation.reason,
            })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct PlanningOnlyNoProgressRule;

impl LoopPolicyRule for PlanningOnlyNoProgressRule {
    fn decide(&self, assessment: &PostTurnAssessment) -> Option<LoopDecision> {
        assessment
            .runtime
            .planning_only_progress
            .then(|| finish(StopReason::NoProgress))
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
    use crate::agent::{ContinueReason, RuntimeEvidence, TextFallbackEvidence, WorkflowEvidence};

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
}
