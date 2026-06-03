use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use imp_llm::{
    AssistantMessage, ContentBlock, Message, Model, StopReason as LlmStopReason, ThinkingLevel,
    Usage,
};
#[cfg(test)]
use imp_llm::{Context, RequestOptions, StreamEvent};
use tokio::sync::mpsc;

use imp_llm::provider::RetryPolicy;

use crate::config::{AgentMode, Config, ContextConfig, ContinuePolicy};
use crate::guardrails::{GuardrailConfig, GuardrailProfile};
use crate::hooks::{HookBackgroundEvent, HookEvent, HookRunner};
use crate::policy::RunPolicy;
use crate::roles::Role;
use crate::tools::{LuaToolLoader, ToolRegistry};
use crate::trace::TraceWriter;
use crate::workflow::WorkflowContract;
use crate::workflow_review::TurnWorkflowReview;

mod autonomy;
mod events;
mod loop_policy;
mod loop_state;
mod subagent;
mod workflow_integration;
pub(super) use workflow_integration::orchestration_follow_up_text;
mod recovery;
mod run_loop;
mod tool_execution;

pub use events::{
    AgentEvent, RecoveryCheckpoint, RecoveryCheckpointKind, TimingEvent, TimingStage,
};
pub use loop_state::{
    ContinueReason, LoopDecision, PlannedToolCall, RunFinalStatus, StopReason, ToolExecutionMode,
    ToolPlan, ToolRisk, TurnPhase, TurnState,
};
pub use recovery::{
    IncompleteToolRecovery, IncompleteToolState, RecoveryLedger, RecoveryReconciliation,
};
pub use subagent::{
    NoopSubagentCoordinator, ParentRunId, SubagentArtifactRef, SubagentCancelResult,
    SubagentConfidence, SubagentContext, SubagentCoordinator, SubagentCoordinatorError,
    SubagentEvent, SubagentFileContext, SubagentInput, SubagentMergePolicy, SubagentMergeResult,
    SubagentOutcome, SubagentPlan, SubagentResourceLimits, SubagentRole, SubagentRunId,
    SubagentSpawnResult, SubagentStatus,
};

/// Commands sent to the agent (from UI or orchestrator).
#[derive(Debug, Clone)]
pub enum AgentCommand {
    Cancel,
    Steer(String),
    FollowUp(String),
}

mod turn_assessment;

use autonomy::{failed_command_recovery_obligation, AutonomousObjective, ObligationLedger};
use turn_assessment::{
    ContinueRecommendation, PostTurnAssessment, RuntimeEvidence, TextFallbackEvidence,
    WorkflowEvidence,
};
pub use turn_assessment::{NextActionAssessment, NextActionDebugView};

/// The core agent — runs the ReAct loop (reason, act, observe).
pub struct Agent {
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: ToolRegistry,
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub cwd: PathBuf,
    pub max_tokens: Option<u32>,
    pub role: Option<Role>,
    pub hooks: HookRunner,
    pub api_key: String,
    /// Optional auth store for automatic OAuth token refresh before LLM calls.
    /// Optional auth store for automatic OAuth token refresh before LLM calls.
    pub auth_store: Option<std::sync::Arc<tokio::sync::Mutex<imp_llm::auth::AuthStore>>>,
    pub ui: Arc<dyn crate::ui::UserInterface>,
    /// Context workflowgement thresholds (wired from Config via AgentBuilder).
    pub context_config: ContextConfig,
    /// Retry policy for transient LLM stream failures.
    pub retry_policy: RetryPolicy,
    /// Active agent mode — controls which tools are permitted.
    pub mode: AgentMode,
    /// Engineering guardrails config.
    pub guardrail_config: GuardrailConfig,
    /// Resolved guardrail profile (None = disabled).
    pub guardrail_profile: Option<GuardrailProfile>,
    /// Cloneable Lua extension tool loader inherited from the session/builder.
    pub lua_tool_loader: Option<LuaToolLoader>,
    /// In-session file content cache, shared across tool calls.
    pub file_cache: Arc<crate::tools::FileCache>,
    /// Shared checkpoint/file-history state, used to capture destructive edit restore points.
    pub checkpoint_state: Arc<crate::tools::CheckpointState>,
    /// Tracks which files have been read; used for staleness and unread-edit warnings.
    pub file_tracker: Arc<std::sync::Mutex<crate::tools::FileTracker>>,
    /// Session-local anchors emitted by read and consumed by anchored edit mode.
    pub anchor_store: Arc<crate::tools::AnchorStore>,
    /// Max lines the read tool may return before truncating. 0 means unlimited.
    pub read_max_lines: usize,
    /// Stable persisted session id for provider-side request grouping.
    pub session_id: Option<String>,
    /// Stable cache namespace id for provider-side prompt caching.
    pub thread_id: Option<String>,
    /// Cache options for LLM requests.
    pub cache_options: imp_llm::CacheOptions,
    /// In-memory recovery checkpoints for this run. Session persistence can seed this ledger later.
    pub recovery_ledger: Arc<std::sync::Mutex<RecoveryLedger>>,
    /// Tracks identical consecutive tool calls to detect loops.
    last_tool_call: std::sync::Arc<std::sync::Mutex<Option<RepeatedToolCallState>>>,
    /// Policy for imp-local visible auto-continuation after high-confidence turns.
    pub continue_policy: ContinuePolicy,
    /// Prevent repeated confidence-based auto-continue nudges in a single run.
    queued_confidence_continue_nudge: bool,
    /// Number of execution-debt stop-gate follow-ups queued in a single run.
    queued_execution_debt_follow_up_count: u8,
    /// Resolved runtime config for tool-specific policy checks.
    pub config: Arc<Config>,
    /// Per-run tool/write policy layered on top of AgentMode.
    pub run_policy: RunPolicy,
    /// Optional host/workflow runtime layer for workflow-backed obligations.
    pub(crate) workflow_layer: workflow_integration::WorkflowRuntimeLayer,

    /// Verification gates declared for this run.
    pub verification_gates: Vec<crate::workflow::VerificationGate>,

    /// Worktree-auto metadata when this run executes in an isolated worktree.
    pub worktree_run_metadata: Option<crate::workflow::WorktreeRunMetadata>,

    /// Active trace writer for the current run artifact, if artifact creation succeeded.
    trace_writer: Arc<Mutex<Option<TraceWriter>>>,
    /// Active run artifact id for trace correlation.
    run_id: Arc<Mutex<Option<String>>>,
    /// Runtime-owned objective for this run, classified from the initial user prompt.
    pub(crate) active_objective: Option<AutonomousObjective>,
    /// Runtime-owned autonomy TODO list used to continue until obligations resolve.
    pub(crate) obligation_ledger: ObligationLedger,

    event_tx: mpsc::UnboundedSender<AgentEvent>,
    command_tx: mpsc::Sender<AgentCommand>,
    command_rx: mpsc::Receiver<AgentCommand>,
    cancel_token: Arc<std::sync::atomic::AtomicBool>,
}

/// Handle for controlling the agent from outside.
pub struct AgentHandle {
    pub event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    pub command_tx: mpsc::Sender<AgentCommand>,
    pub cancel_token: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone)]
struct RepeatedToolCallState {
    tool_name: String,
    args_json: String,
    consecutive: usize,
}

#[derive(Debug, Clone)]
enum RepeatedToolCallCheck {
    Ok,
    Warn(String),
    Block(imp_llm::ToolResultMessage),
}

impl Agent {
    pub fn new(model: Model, cwd: PathBuf) -> (Self, AgentHandle) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::channel(32);
        let cancel_token = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut hooks = HookRunner::new();
        let background_event_tx = event_tx.clone();
        hooks.set_background_reporter(Arc::new(move |event: HookBackgroundEvent| {
            let background_event_tx = background_event_tx.clone();
            tokio::spawn(async move {
                let _ = background_event_tx.send(AgentEvent::Warning {
                    message: event.to_string(),
                });
            });
        }));

        let agent = Self {
            model,
            thinking_level: ThinkingLevel::Medium,
            tools: ToolRegistry::new(),
            messages: Vec::new(),
            system_prompt: String::new(),
            cwd: cwd.clone(),
            max_tokens: None,
            role: None,
            hooks,
            api_key: String::new(),
            ui: Arc::new(crate::ui::NullInterface),
            context_config: ContextConfig::default(),
            retry_policy: RetryPolicy::default(),
            mode: AgentMode::Full,
            guardrail_config: GuardrailConfig::default(),
            guardrail_profile: None,
            file_cache: Arc::new(crate::tools::FileCache::new()),
            checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
            anchor_store: Arc::new(crate::tools::AnchorStore::new()),
            read_max_lines: 500,
            auth_store: None,
            session_id: None,
            thread_id: None,
            cache_options: imp_llm::CacheOptions {
                cache_system_prompt: true,
                cache_tools: true,
                cache_recent_turns: 2,
                extended_ttl: false,
                global_scope: false,
            },
            recovery_ledger: Arc::new(std::sync::Mutex::new(RecoveryLedger::new())),
            last_tool_call: Arc::new(std::sync::Mutex::new(None)),
            continue_policy: ContinuePolicy::Disabled,
            queued_confidence_continue_nudge: false,
            queued_execution_debt_follow_up_count: 0,
            config: Arc::new(Config::default()),
            run_policy: RunPolicy::default(),
            workflow_layer: workflow_integration::WorkflowRuntimeLayer::new(
                WorkflowContract::implicit_from(
                    crate::workflow::ImplicitWorkflowContractInput::prompt("").cwd(&cwd),
                ),
            ),
            verification_gates: Vec::new(),
            worktree_run_metadata: None,
            trace_writer: Arc::new(Mutex::new(None)),
            run_id: Arc::new(Mutex::new(None)),
            active_objective: None,
            obligation_ledger: ObligationLedger::default(),
            lua_tool_loader: None,

            event_tx,
            command_tx: command_tx.clone(),
            command_rx,
            cancel_token: Arc::clone(&cancel_token),
        };

        let handle = AgentHandle {
            event_rx,
            command_tx,
            cancel_token,
        };

        (agent, handle)
    }

    fn assess_post_turn(
        &self,
        message: &AssistantMessage,
        tool_results: &[imp_llm::ToolResultMessage],
        _used_tools: bool,
        workflow_review: &TurnWorkflowReview,
    ) -> PostTurnAssessment {
        let repeated_action = tool_results_indicate_repeated_action(tool_results);
        let runtime_execution_stop_reason =
            tool_results_indicate_execution_blocker(tool_results, self.mode);
        let work_completed = tool_results_indicate_work_completed(tool_results, self.mode);
        let workflow_signals = self.workflow_post_turn_signals(tool_results, workflow_review);
        let planning_only_progress =
            workflow_signals.execution_debt && !workflow_signals.execution_evidence;
        let workflow_stop_reason = workflow_signals.stop_reason;
        let planner_text_stop_reason = None;

        let failed_bash_needs_recovery =
            tool_results_indicate_failed_bash_command(tool_results, self.mode)
                && self.queued_execution_debt_follow_up_count == 0;
        let mut obligation_ledger = self.obligation_ledger.clone();
        if failed_bash_needs_recovery {
            obligation_ledger.add(failed_command_recovery_obligation());
        }
        let execution_text_stop_reason = None;
        let continue_recommendation = if let Some(recommendation) =
            self.workflow_continue_recommendation(&workflow_signals)
        {
            Some(recommendation)
        } else if failed_bash_needs_recovery {
            Some(ContinueRecommendation {
                prompt: failed_bash_recovery_follow_up_text().to_string(),
                reason: ContinueReason::ExecutionDebt,
            })
        } else if let Some((prompt, reason)) = obligation_ledger.next_continue() {
            Some(ContinueRecommendation { prompt, reason })
        } else if self.should_retry_unanswered_execution_debt(
            tool_results,
            workflow_signals.execution_evidence,
        ) {
            Some(ContinueRecommendation {
                prompt: execution_debt_follow_up_text().to_string(),
                reason: ContinueReason::ExecutionDebt,
            })
        } else if let Some(prompt) = self.workflow_externalization_follow_up(message) {
            Some(ContinueRecommendation {
                prompt: prompt.to_string(),
                reason: ContinueReason::ExternalizationNeeded,
            })
        } else if !matches!(self.mode, AgentMode::Planner)
            && should_queue_execution_debt_follow_up(
                workflow_signals.execution_debt,
                workflow_signals.execution_evidence,
                self.queued_execution_debt_follow_up_count > 0,
                !assistant_message_text(message).trim().is_empty(),
            )
        {
            Some(ContinueRecommendation {
                prompt: execution_debt_follow_up_text().to_string(),
                reason: ContinueReason::ExecutionDebt,
            })
        } else if should_queue_confidence_continue_follow_up(
            message,
            self.mode,
            self.continue_policy,
            self.queued_confidence_continue_nudge,
        ) {
            Some(ContinueRecommendation {
                prompt: confidence_continue_follow_up_text().to_string(),
                reason: ContinueReason::HighConfidenceVisibleNextStep,
            })
        } else {
            None
        };

        PostTurnAssessment {
            runtime: RuntimeEvidence {
                repeated_action,
                execution_stop_reason: runtime_execution_stop_reason,
                work_completed,
                execution_debt: workflow_signals.execution_debt,
                execution_evidence: workflow_signals.execution_evidence,
                planning_only_progress,
                orchestration_started: workflow_signals.orchestration_started,
            },
            workflow: WorkflowEvidence {
                stop_reason: workflow_stop_reason,
            },
            text_fallback: TextFallbackEvidence {
                planner_stop_reason: planner_text_stop_reason,
                execution_stop_reason: execution_text_stop_reason,
            },
            continue_recommendation,
        }
    }

    fn mark_continue_reason(&mut self, reason: ContinueReason) {
        match reason {
            ContinueReason::ExternalizationNeeded => {
                self.mark_workflow_externalization_nudge_queued();
            }
            ContinueReason::HighConfidenceVisibleNextStep => {
                self.queued_confidence_continue_nudge = true;
            }
            ContinueReason::ExecutionDebt => {
                self.queued_execution_debt_follow_up_count += 1;
                self.obligation_ledger
                    .resolve_kind(autonomy::ObligationKind::FailedCommandRecovery);
                self.obligation_ledger
                    .resolve_kind(autonomy::ObligationKind::EditedFilesVerification);
            }
            ContinueReason::ToolResultsNeedInterpretation
            | ContinueReason::QueuedUserFollowUp
            | ContinueReason::OrchestrationProgress
            | ContinueReason::WorkflowProgress
            | ContinueReason::WorkflowCloseout
            | ContinueReason::WorkflowBootstrap
            | ContinueReason::WorkflowDecomposition => {}
        }
    }

    pub(crate) async fn emit(&self, event: AgentEvent) {
        // Fire corresponding hooks for lifecycle events
        match &event {
            AgentEvent::AgentEnd { .. } => {
                self.hooks
                    .fire(&HookEvent::OnAgentEnd {
                        messages: &self.messages,
                    })
                    .await;
            }
            AgentEvent::TurnEnd { index, message, .. } => {
                self.hooks
                    .fire(&HookEvent::OnTurnEnd {
                        index: *index,
                        message,
                    })
                    .await;
            }
            _ => {}
        }
        self.write_trace_event(&event);
        let _ = self.event_tx.send(event);
    }

    fn write_trace_event(&self, event: &AgentEvent) {
        let Some(run_id) = self.run_id.lock().ok().and_then(|run_id| run_id.clone()) else {
            return;
        };
        let mut trace_event = event.to_trace_event(run_id);
        if let Some(workflow_id) = self
            .workflow_contract()
            .id
            .as_ref()
            .or(self.workflow_contract().workflow_unit_ref.as_ref())
        {
            trace_event = trace_event.with_workflow_id(workflow_id.clone());
        }
        if let Ok(mut writer) = self.trace_writer.lock() {
            if let Some(writer) = writer.as_mut() {
                let _ = writer.write_event(trace_event);
                let _ = writer.flush();
            }
        }
    }

    async fn emit_timing(
        &self,
        turn: u32,
        stage: TimingStage,
        turn_started_at: Instant,
        llm_request_started_at: Option<Instant>,
    ) {
        self.emit_timing_with_details(TimingEvent::new(
            turn,
            stage,
            turn_started_at,
            llm_request_started_at,
        ))
        .await;
    }

    async fn emit_timing_with_details(&self, timing: TimingEvent) {
        self.write_trace_event(&AgentEvent::Timing {
            timing: timing.clone(),
        });
        let _ = self.event_tx.send(AgentEvent::Timing { timing });
    }

    pub async fn emit_recovery_checkpoint(&self, checkpoint: RecoveryCheckpoint) {
        if let Ok(mut ledger) = self.recovery_ledger.lock() {
            ledger.record(checkpoint.clone());
        }
        self.write_trace_event(&AgentEvent::RecoveryCheckpoint {
            checkpoint: checkpoint.clone(),
        });
        let _ = self
            .event_tx
            .send(AgentEvent::RecoveryCheckpoint { checkpoint });
    }

    fn recovery_checkpoint(
        turn: u32,
        kind: RecoveryCheckpointKind,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        args_hash: Option<String>,
        success: Option<bool>,
        error_class: Option<String>,
    ) -> RecoveryCheckpoint {
        RecoveryCheckpoint {
            version: 1,
            turn,
            kind,
            tool_call_id,
            tool_name,
            args_hash,
            success,
            error_class,
            timestamp: imp_llm::now(),
        }
    }

    fn tool_args_hash(args: &serde_json::Value) -> String {
        format!("{:016x}", crate::tools::stable_hash(args.to_string()))
    }
}
fn push_stream_text_block(content: &mut Vec<ContentBlock>, text: String) {
    if text.is_empty() {
        return;
    }

    if let Some(ContentBlock::Text { text: existing }) = content.last_mut() {
        existing.push_str(&text);
    } else {
        content.push(ContentBlock::Text { text });
    }
}

fn push_stream_thinking_block(content: &mut Vec<ContentBlock>, text: String) {
    if text.is_empty() {
        return;
    }

    if let Some(ContentBlock::Thinking { text: existing }) = content.last_mut() {
        existing.push_str(&text);
    } else {
        content.push(ContentBlock::Thinking { text });
    }
}

fn assistant_message_text(message: &AssistantMessage) -> String {
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

fn assistant_message_contains_workflow_tool_call(message: &AssistantMessage) -> bool {
    message.content.iter().any(|block| match block {
        ContentBlock::ToolCall { name, .. } => name == "workflow",
        _ => false,
    })
}

fn should_queue_execution_debt_follow_up(
    execution_debt: bool,
    execution_evidence: bool,
    already_queued: bool,
    assistant_finalized: bool,
) -> bool {
    execution_debt && !execution_evidence && !already_queued && assistant_finalized
}

fn should_queue_confidence_continue_follow_up(
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

fn confidence_continue_follow_up_text() -> &'static str {
    "Confidence is high and the workflow delta is already visible. Continue to the next small, well-bounded step now using the native workflow-backed process, unless a consequential decision or blocker appears. Do not re-summarize the same visible workflow change in chat unless new context needs to be called out."
}

fn failed_bash_recovery_follow_up_text() -> &'static str {
    "The last bash command failed, but a failed command is usually diagnostic evidence, not a stopping condition. Inspect the command output, identify the root cause, make the smallest useful fix or choose a better command, and rerun the relevant check. Stop only if the failure proves a concrete blocker that needs user input."
}

fn execution_debt_follow_up_text() -> &'static str {
    "You have recorded or planned work, but the requested outcome is not satisfied yet. Continue working until the user's requested outcome is satisfied, or until concrete evidence shows it cannot be completed. Do not stop merely because you recorded a plan, updated a workflow, or completed one intermediate step."
}

fn tool_results_include_successful_edit(tool_results: &[imp_llm::ToolResultMessage]) -> bool {
    tool_results.iter().any(|result| {
        !result.is_error && matches!(result.tool_name.as_str(), "write" | "edit" | "multi_edit")
    })
}

fn tool_results_include_successful_check(tool_results: &[imp_llm::ToolResultMessage]) -> bool {
    tool_results.iter().any(|result| {
        matches!(result.tool_name.as_str(), "bash" | "shell")
            && bash_result_is_successful_check(result)
    })
}

fn tool_results_indicate_repeated_action(tool_results: &[imp_llm::ToolResultMessage]) -> bool {
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

fn tool_results_indicate_failed_bash_command(
    tool_results: &[imp_llm::ToolResultMessage],
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

fn tool_results_indicate_execution_blocker(
    tool_results: &[imp_llm::ToolResultMessage],
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

fn bash_result_is_successful_check(result: &imp_llm::ToolResultMessage) -> bool {
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

fn tool_results_indicate_work_completed(
    tool_results: &[imp_llm::ToolResultMessage],
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

/// Build an AssistantMessage from accumulated stream parts while preserving
/// the original block order emitted by the model.
fn build_assistant_message(
    content: &[ContentBlock],
    tool_calls: &[(String, String, serde_json::Value)],
    usage: Option<Usage>,
) -> AssistantMessage {
    let stop_reason = if tool_calls.is_empty() {
        LlmStopReason::EndTurn
    } else {
        LlmStopReason::ToolUse
    };

    AssistantMessage {
        content: content.to_vec(),
        usage,
        stop_reason,
        timestamp: imp_llm::now(),
    }
}

fn clone_model(model: &Model) -> Model {
    Model {
        meta: model.meta.clone(),
        provider: Arc::clone(&model.provider),
    }
}

fn extract_file_path(cwd: &Path, args: &serde_json::Value) -> Option<PathBuf> {
    let raw_path = args.get("path")?.as_str()?;
    if raw_path.is_empty() {
        return None;
    }

    let path = PathBuf::from(raw_path);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(cwd.join(path))
    }
}

#[cfg(test)]
mod tests;
// ── Integration tests: full ReAct cycle with real tools ─────────────

#[cfg(test)]
#[path = "integration_tests.rs"]
mod integration;
// ── Mode enforcement tests ─────────────────────────────────────────

#[cfg(test)]
mod mode_tests {
    use super::*;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::auth::{ApiKey, AuthStore};
    use imp_llm::model::ModelMeta;
    use imp_llm::provider::Provider;
    use tokio::sync::Mutex;

    // ── Mock provider (same shape as in tests) ─────────────────────

    struct MockProvider {
        responses: Mutex<Vec<Vec<imp_llm::StreamEvent>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Vec<imp_llm::StreamEvent>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn stream(
            &self,
            _model: &imp_llm::Model,
            _context: imp_llm::Context,
            _options: imp_llm::RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<imp_llm::StreamEvent>> + Send>> {
            let mut responses = self.responses.try_lock().expect("MockProvider lock");
            let events = if responses.is_empty() {
                vec![imp_llm::StreamEvent::Error {
                    error: "No more mock responses".to_string(),
                }]
            } else {
                responses.remove(0)
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("mock-key".to_string())
        }

        fn id(&self) -> &str {
            "mock"
        }

        fn models(&self) -> &[imp_llm::model::ModelMeta] {
            &[]
        }
    }

    fn test_model(provider: Arc<dyn Provider>) -> imp_llm::Model {
        imp_llm::Model {
            meta: ModelMeta {
                id: "test-model".to_string(),
                provider: "mock".to_string(),
                name: "Test Model".to_string(),
                context_window: 200_000,
                max_output_tokens: 16_384,
                pricing: imp_llm::model::ModelPricing {
                    input_per_mtok: 3.0,
                    output_per_mtok: 15.0,
                    cache_read_per_mtok: 0.3,
                    cache_write_per_mtok: 3.75,
                },
                capabilities: imp_llm::model::Capabilities {
                    reasoning: true,
                    images: false,
                    tool_use: true,
                },
            },
            provider,
        }
    }

    fn text_response(text: &str, input: u32, output: u32) -> Vec<imp_llm::StreamEvent> {
        vec![
            imp_llm::StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            imp_llm::StreamEvent::TextDelta {
                text: text.to_string(),
            },
            imp_llm::StreamEvent::MessageEnd {
                message: imp_llm::AssistantMessage {
                    content: vec![imp_llm::ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    usage: Some(imp_llm::Usage {
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: imp_llm::StopReason::EndTurn,
                    timestamp: 1000,
                },
            },
        ]
    }

    fn tool_call_response(
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        input: u32,
        output: u32,
    ) -> Vec<imp_llm::StreamEvent> {
        vec![
            imp_llm::StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            imp_llm::StreamEvent::ToolCall {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments: args.clone(),
            },
            imp_llm::StreamEvent::MessageEnd {
                message: imp_llm::AssistantMessage {
                    content: vec![imp_llm::ContentBlock::ToolCall {
                        id: call_id.to_string(),
                        name: tool_name.to_string(),
                        arguments: args,
                    }],
                    usage: Some(imp_llm::Usage {
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: imp_llm::StopReason::ToolUse,
                    timestamp: 1000,
                },
            },
        ]
    }

    async fn collect_events(mut handle: AgentHandle) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = handle.event_rx.recv().await {
            events.push(event);
        }
        events
    }

    // ── Tool fixtures ───────────────────────────────────────────────

    struct EchoTool;

    #[async_trait]
    impl crate::tools::Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "Echo"
        }
        fn description(&self) -> &str {
            "Echoes back the input"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }
        fn is_readonly(&self) -> bool {
            true
        }
        async fn execute(
            &self,
            _call_id: &str,
            params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            let text = params["text"].as_str().unwrap_or("no text");
            Ok(crate::tools::ToolOutput::text(format!("echo: {text}")))
        }
    }

    struct NamedWriteTool(&'static str);

    #[async_trait]
    impl crate::tools::Tool for NamedWriteTool {
        fn name(&self) -> &str {
            self.0
        }
        fn label(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "A write tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"data": {"type": "string"}}})
        }
        fn is_readonly(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _call_id: &str,
            _params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::text("written"))
        }
    }

    fn single_text_model(text: &str) -> Arc<MockProvider> {
        Arc::new(MockProvider::new(vec![text_response(text, 50, 10)]))
    }

    /// Test: Full mode registers all tools (no filtering).
    #[tokio::test]
    async fn agent_mode_enforcement_full_registers_all_tools() {
        use crate::config::AgentMode;

        let provider = single_text_model("ok");
        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;

        // Register a mix of tools
        agent.tools.register(Arc::new(EchoTool)); // "echo" - not in any allow-list
        agent.tools.register(Arc::new(NamedWriteTool("write")));

        // Full mode allows everything — both tools should be present
        assert!(
            agent.tools.get("echo").is_some(),
            "echo should be registered"
        );
        assert!(
            agent.tools.get("write").is_some(),
            "write should be registered"
        );
        assert!(agent.mode.allows_tool("echo"));
        assert!(agent.mode.allows_tool("write"));
        assert!(agent.mode.allows_tool("any_future_tool"));
    }

    /// Test: Orchestrator mode excludes write-category tools at registration time.
    #[test]
    fn agent_mode_enforcement_orchestrator_excludes_write_tools() {
        use crate::config::AgentMode;
        use crate::tools::ToolRegistry;

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)); // "echo" — not in orchestrator allow-list
        registry.register(Arc::new(NamedWriteTool("write")));
        registry.register(Arc::new(NamedWriteTool("edit")));
        registry.register(Arc::new(NamedWriteTool("bash")));

        // Apply the mode filter exactly as AgentBuilder would
        let mode = AgentMode::Orchestrator;
        registry.retain(|name| mode.allows_tool(name));

        // Write-category tools must be absent
        assert!(
            registry.get("write").is_none(),
            "write must be filtered out"
        );
        assert!(registry.get("edit").is_none(), "edit must be filtered out");
        assert!(registry.get("bash").is_none(), "bash must be filtered out");
        // echo is not in any mode allow-list either
        assert!(registry.get("echo").is_none(), "echo must be filtered out");
    }

    /// Test: Execution-time guard blocks a disallowed tool call and returns an error result.
    #[tokio::test]
    async fn agent_mode_enforcement_guard_blocks_disallowed() {
        use crate::config::AgentMode;

        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: model calls "write" — disallowed in orchestrator mode
            tool_call_response(
                "call_1",
                "write",
                serde_json::json!({"data": "content"}),
                50,
                10,
            ),
            // Turn 1: model responds after seeing the error
            text_response("Understood, I cannot write directly.", 50, 10),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Orchestrator;
        // Register write so it passes schema validation — the mode guard fires first
        agent.tools.register(Arc::new(NamedWriteTool("write")));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Write something".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // The tool execution end event should carry an error result
        let tool_end = events
            .iter()
            .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
        assert!(tool_end.is_some(), "should have a ToolExecutionEnd event");

        if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
            assert!(result.is_error, "mode guard should produce an error result");
            let text = result.content.iter().find_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            });
            let text = text.expect("error result should have text");
            assert!(
                text.contains("write") && text.contains("mode"),
                "error should name the tool and mention mode, got: {text}"
            );
        }
    }

    /// Test: Execution-time guard allows a permitted tool call through cleanly.
    #[tokio::test]
    async fn agent_mode_enforcement_guard_allows_permitted() {
        use crate::config::AgentMode;

        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: model calls "read" — allowed in orchestrator mode
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                50,
                10,
            ),
            text_response("Echo succeeded", 50, 10),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        // Full mode keeps custom tools available
        agent.mode = AgentMode::Full;
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo something".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // Tool should have succeeded (not an error)
        let tool_end = events
            .iter()
            .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
        assert!(tool_end.is_some());

        if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
            assert!(!result.is_error, "permitted tool should succeed");
        }
    }

    /// Test: System prompt filters tool descriptions by mode.
    #[test]
    fn agent_mode_enforcement_system_prompt_filters() {
        use crate::config::AgentMode;
        use crate::system_prompt::{assemble, AssembleParams};
        use crate::tools::ToolRegistry;

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NamedWriteTool("write")));
        registry.register(Arc::new(NamedWriteTool("edit")));
        registry.register(Arc::new(NamedWriteTool("bash")));

        // Provide read-category tools too
        struct ReadTool;
        #[async_trait]
        impl crate::tools::Tool for ReadTool {
            fn name(&self) -> &str {
                "read"
            }
            fn label(&self) -> &str {
                "Read"
            }
            fn description(&self) -> &str {
                "Read a file"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            fn is_readonly(&self) -> bool {
                true
            }
            async fn execute(
                &self,
                _: &str,
                _: serde_json::Value,
                _: crate::tools::ToolContext,
            ) -> crate::error::Result<crate::tools::ToolOutput> {
                Ok(crate::tools::ToolOutput::text(""))
            }
        }
        registry.register(Arc::new(ReadTool));

        let mode = AgentMode::Orchestrator;
        let result = assemble(&AssembleParams {
            tools: &registry,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &mode,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        // Orchestrator allows "read" — should appear in system prompt
        assert!(
            result.text.contains("- read:"),
            "read should be in orchestrator prompt"
        );

        // Write tools must be absent from the system prompt
        assert!(
            !result.text.contains("- write:"),
            "write must not appear in orchestrator prompt"
        );
        assert!(
            !result.text.contains("- edit:"),
            "edit must not appear in orchestrator prompt"
        );
        assert!(
            !result.text.contains("- bash:"),
            "bash must not appear in orchestrator prompt"
        );
    }

    /// Test: System prompt includes mode instructions for non-Full modes.
    #[test]
    fn agent_mode_enforcement_system_prompt_instructions() {
        use crate::config::AgentMode;
        use crate::system_prompt::{assemble, AssembleParams};
        use crate::tools::ToolRegistry;

        let registry = ToolRegistry::new();

        // Full mode — no extra instructions
        let full_result = assemble(&AssembleParams {
            tools: &registry,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        // Full mode has no instructions
        assert!(
            !full_result.text.contains("orchestrator"),
            "Full mode should not mention orchestrator"
        );
        assert!(
            !full_result.text.contains("You are a worker agent."),
            "Full mode should not include worker mode instructions"
        );

        // Orchestrator mode — should include mode instructions
        let orch_result = assemble(&AssembleParams {
            tools: &registry,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Orchestrator,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(
            orch_result.text.contains("orchestrator"),
            "orchestrator prompt should contain mode instructions, got: {}",
            orch_result.text
        );

        // Worker mode — should include mode instructions
        let worker_result = assemble(&AssembleParams {
            tools: &registry,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Worker,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(
            worker_result.text.contains("worker"),
            "worker prompt should contain mode instructions"
        );

        // Reviewer mode — should include mode instructions
        let reviewer_result = assemble(&AssembleParams {
            tools: &registry,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Reviewer,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(
            reviewer_result.text.contains("reviewer") || reviewer_result.text.contains("read"),
            "reviewer prompt should contain mode instructions"
        );
    }
}
