use imp_llm::{AssistantMessage, ContentBlock, ToolResultMessage};

use crate::config::{AgentMode, ContinuePolicy};

use super::{ContinueReason, StopReason};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NextAction {
    Continue {
        prompt: String,
        reason: ContinueReason,
    },
    Stop {
        reason: NextActionStopReason,
    },
}

pub(super) type NextActionStopReason = StopReason;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeEvidence {
    pub(super) repeated_action: bool,
    pub(super) execution_stop_reason: Option<NextActionStopReason>,
    pub(super) work_completed: bool,
    pub(super) execution_debt: bool,
    pub(super) execution_evidence: bool,
    pub(super) planning_only_progress: bool,
    pub(super) orchestration_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkflowEvidence {
    pub(super) stop_reason: Option<NextActionStopReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TextFallbackEvidence {
    pub(super) planner_stop_reason: Option<NextActionStopReason>,
    pub(super) execution_stop_reason: Option<NextActionStopReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContinueRecommendation {
    pub(super) prompt: String,
    pub(super) reason: ContinueReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextActionAssessment {
    pub runtime: NextActionRuntimeEvidence,
    pub workflow: NextActionWorkflowEvidence,
    pub text_fallback: NextActionTextFallbackEvidence,
    pub continue_recommendation: Option<NextActionContinueRecommendation>,
    pub chosen_action: NextActionDebugView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextActionRuntimeEvidence {
    pub repeated_action: bool,
    pub execution_stop_reason: Option<String>,
    pub work_completed: bool,
    pub execution_debt: bool,
    pub execution_evidence: bool,
    pub planning_only_progress: bool,
    pub orchestration_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextActionWorkflowEvidence {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextActionTextFallbackEvidence {
    pub planner_stop_reason: Option<String>,
    pub execution_stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextActionContinueRecommendation {
    pub prompt: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextActionDebugView {
    Continue { prompt: String, reason: String },
    Stop { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostTurnAssessment {
    pub(super) runtime: RuntimeEvidence,
    pub(super) workflow: WorkflowEvidence,
    pub(super) text_fallback: TextFallbackEvidence,
    pub(super) continue_recommendation: Option<ContinueRecommendation>,
}

impl PostTurnAssessment {
    pub(super) fn into_next_action(self) -> NextAction {
        if self.runtime.repeated_action {
            return NextAction::Stop {
                reason: NextActionStopReason::RepeatedAction,
            };
        }

        if let Some(reason) = self.runtime.execution_stop_reason {
            return NextAction::Stop { reason };
        }

        if self.runtime.work_completed && !self.runtime.orchestration_started {
            return NextAction::Stop {
                reason: NextActionStopReason::WorkCompleted,
            };
        }

        if let Some(reason) = self.workflow.stop_reason {
            return NextAction::Stop { reason };
        }

        if let Some(reason) = self.text_fallback.planner_stop_reason {
            return NextAction::Stop { reason };
        }

        if let Some(reason) = self.text_fallback.execution_stop_reason {
            return NextAction::Stop { reason };
        }

        if let Some(continue_recommendation) = self.continue_recommendation {
            return NextAction::Continue {
                prompt: continue_recommendation.prompt,
                reason: continue_recommendation.reason,
            };
        }

        if self.runtime.orchestration_started {
            return NextAction::Continue {
                prompt: super::orchestration_follow_up_text(None),
                reason: ContinueReason::OrchestrationProgress,
            };
        }

        if self.runtime.planning_only_progress {
            return NextAction::Stop {
                reason: NextActionStopReason::NoProgress,
            };
        }

        NextAction::Stop {
            reason: NextActionStopReason::NoAutomaticFollowUp,
        }
    }

    pub(super) fn debug_view(&self) -> NextActionAssessment {
        let chosen_action = match self.clone().into_next_action() {
            NextAction::Continue { prompt, reason } => NextActionDebugView::Continue {
                prompt,
                reason: reason.as_str().to_string(),
            },
            NextAction::Stop { reason } => NextActionDebugView::Stop {
                reason: reason.as_str().to_string(),
            },
        };

        NextActionAssessment {
            runtime: NextActionRuntimeEvidence {
                repeated_action: self.runtime.repeated_action,
                execution_stop_reason: self
                    .runtime
                    .execution_stop_reason
                    .map(|reason| reason.as_str().to_string()),
                work_completed: self.runtime.work_completed,
                execution_debt: self.runtime.execution_debt,
                execution_evidence: self.runtime.execution_evidence,
                planning_only_progress: self.runtime.planning_only_progress,
                orchestration_started: self.runtime.orchestration_started,
            },
            workflow: NextActionWorkflowEvidence {
                stop_reason: self
                    .workflow
                    .stop_reason
                    .map(|reason| reason.as_str().to_string()),
            },
            text_fallback: NextActionTextFallbackEvidence {
                planner_stop_reason: self
                    .text_fallback
                    .planner_stop_reason
                    .map(|reason| reason.as_str().to_string()),
                execution_stop_reason: self
                    .text_fallback
                    .execution_stop_reason
                    .map(|reason| reason.as_str().to_string()),
            },
            continue_recommendation: self.continue_recommendation.clone().map(|recommendation| {
                NextActionContinueRecommendation {
                    prompt: recommendation.prompt,
                    reason: recommendation.reason.as_str().to_string(),
                }
            }),
            chosen_action,
        }
    }
}

pub(super) fn assistant_message_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn assistant_message_contains_workflow_tool_call(message: &AssistantMessage) -> bool {
    message.content.iter().any(|block| match block {
        ContentBlock::ToolCall { name, .. } => name == "workflow",
        _ => false,
    })
}

pub(super) fn should_queue_execution_debt_follow_up(
    execution_debt: bool,
    execution_evidence: bool,
    already_queued: bool,
    assistant_finalized: bool,
) -> bool {
    execution_debt && !execution_evidence && !already_queued && assistant_finalized
}

pub(super) fn should_queue_confidence_continue_follow_up(
    message: &AssistantMessage,
    mode: AgentMode,
    continue_policy: ContinuePolicy,
    already_queued: bool,
) -> bool {
    if already_queued || matches!(continue_policy, ContinuePolicy::Disabled) {
        return false;
    }

    if !matches!(
        mode,
        AgentMode::Full | AgentMode::Planner | AgentMode::Orchestrator
    ) {
        return false;
    }

    if !assistant_message_contains_workflow_tool_call(message) {
        return false;
    }

    let text = assistant_message_text(message);
    if text.trim().is_empty() {
        return false;
    }

    let lower = text.to_ascii_lowercase();
    let positive_signal = [
        "done",
        "completed",
        "finished",
        "updated",
        "created",
        "next",
        "continue",
        "proceed",
        "follow-up",
        "follow up",
    ]
    .iter()
    .filter(|needle| lower.contains(**needle))
    .count();

    let blocker_signal = [
        "blocked",
        "unclear",
        "need your input",
        "which should",
        "approval",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if blocker_signal {
        return false;
    }

    let threshold = match continue_policy {
        ContinuePolicy::Disabled => return false,
        ContinuePolicy::Conservative => 3,
        ContinuePolicy::Balanced => 2,
        ContinuePolicy::Aggressive => 1,
    };

    positive_signal >= threshold
}

pub(super) fn tool_results_include_successful_edit(tool_results: &[ToolResultMessage]) -> bool {
    tool_results.iter().any(|result| {
        !result.is_error && matches!(result.tool_name.as_str(), "write" | "edit" | "multi_edit")
    })
}

pub(super) fn tool_results_include_successful_check(tool_results: &[ToolResultMessage]) -> bool {
    tool_results.iter().any(|result| {
        matches!(result.tool_name.as_str(), "bash" | "shell")
            && bash_result_is_successful_check(result)
    })
}

pub(super) fn tool_results_indicate_repeated_action(tool_results: &[ToolResultMessage]) -> bool {
    tool_results.iter().any(|result| {
        result.is_error
            && result.content.iter().any(|block| match block {
                ContentBlock::Text { text } => {
                    text.contains("Blocked: identical tool call repeated")
                }
                _ => false,
            })
    })
}

pub(super) fn tool_results_indicate_failed_bash_command(
    tool_results: &[ToolResultMessage],
    mode: AgentMode,
) -> bool {
    if !matches!(
        mode,
        AgentMode::Full | AgentMode::Orchestrator | AgentMode::Worker
    ) {
        return false;
    }

    tool_results.iter().any(|result| {
        if !(result.tool_name == "bash" || result.tool_name == "shell") {
            return false;
        }
        let exit_code = result.details.get("exit_code").and_then(|v| v.as_i64());
        let timed_out = result.details.get("timed_out").and_then(|v| v.as_bool()) == Some(true);
        let cancelled = result.details.get("cancelled").and_then(|v| v.as_bool()) == Some(true);
        result.is_error || timed_out || cancelled || exit_code.is_some_and(|code| code != 0)
    })
}

pub(super) fn tool_results_indicate_execution_blocker(
    tool_results: &[ToolResultMessage],
    mode: AgentMode,
) -> Option<StopReason> {
    if !matches!(
        mode,
        AgentMode::Full | AgentMode::Orchestrator | AgentMode::Worker
    ) {
        return None;
    }

    for result in tool_results {
        let action = result.details.get("action").and_then(|v| v.as_str());

        if action == Some("verify")
            && result.details.get("passed").and_then(|v| v.as_bool()) == Some(false)
        {
            return Some(StopReason::ExecutionBlocked);
        }

        if result.tool_name == "bash" || result.tool_name == "shell" {
            let exit_code = result.details.get("exit_code").and_then(|v| v.as_i64());
            let timed_out = result.details.get("timed_out").and_then(|v| v.as_bool()) == Some(true);
            let cancelled = result.details.get("cancelled").and_then(|v| v.as_bool()) == Some(true);
            let command = result
                .details
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let looks_like_check = command.contains("check")
                || command.contains("test")
                || command.contains("verify")
                || command.contains("pytest")
                || command.contains("cargo test")
                || command.contains("cargo check");

            if looks_like_check
                && (timed_out || cancelled || exit_code.is_some_and(|code| code != 0))
            {
                continue;
            }

            if timed_out || cancelled || exit_code.is_some_and(|code| code != 0) {
                continue;
            }
        }
    }

    None
}

pub(super) fn bash_result_is_successful_check(result: &ToolResultMessage) -> bool {
    if result.is_error {
        return false;
    }
    let Some(command) = result.details.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let exit_code_ok = result.details.get("exit_code").and_then(|v| v.as_i64()) == Some(0);
    if !exit_code_ok {
        return false;
    }
    let command = command.to_ascii_lowercase();
    command.contains("check")
        || command.contains("test")
        || command.contains("verify")
        || command.contains("pytest")
        || command.contains("cargo test")
        || command.contains("cargo check")
}

pub(super) fn tool_results_indicate_work_completed(
    tool_results: &[ToolResultMessage],
    mode: AgentMode,
) -> bool {
    if !matches!(
        mode,
        AgentMode::Full | AgentMode::Orchestrator | AgentMode::Worker
    ) {
        return false;
    }

    let mut saw_edit_like_success = false;
    let mut saw_successful_check = false;

    for result in tool_results {
        if result.is_error {
            continue;
        }

        if matches!(result.tool_name.as_str(), "write" | "edit" | "multi_edit") {
            saw_edit_like_success = true;
        }
        if result.tool_name == "read" && saw_edit_like_success {
            return true;
        }

        if result.tool_name == "workflow" {
            continue;
        }

        if let Some(command) = result.details.get("command").and_then(|v| v.as_str()) {
            let exit_code_ok = result.details.get("exit_code").and_then(|v| v.as_i64()) == Some(0);
            let command_lower = command.to_ascii_lowercase();
            let looks_like_check = command_lower.contains("check")
                || command_lower.contains("test")
                || command_lower.contains("verify")
                || command_lower.contains("pytest")
                || command_lower.contains("cargo test")
                || command_lower.contains("cargo check");
            if exit_code_ok && looks_like_check {
                saw_successful_check = true;
            }
        }
    }

    saw_edit_like_success && saw_successful_check
}
