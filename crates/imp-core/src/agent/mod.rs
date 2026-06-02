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
mod tests {
    use super::*;
    use crate::agent::turn_assessment::NextAction;
    use crate::builder::AgentBuilder;
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex as StdMutex,
    };
    use std::time::Duration;

    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::auth::{ApiKey, AuthStore};
    use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
    use imp_llm::provider::Provider;
    use imp_llm::ToolResultMessage;
    use tokio::sync::{Mutex, Notify};

    #[derive(Default)]
    struct AnsweringUi {
        answer: String,
    }

    #[async_trait]
    impl crate::ui::UserInterface for AnsweringUi {
        fn has_ui(&self) -> bool {
            true
        }

        async fn notify(&self, _: &str, _: crate::ui::NotifyLevel) {}

        async fn confirm(&self, _: &str, _: &str) -> Option<bool> {
            None
        }

        async fn select_with_context(
            &self,
            _: &str,
            _: &str,
            _: &[crate::ui::SelectOption],
        ) -> Option<usize> {
            None
        }

        async fn multi_select_with_context(
            &self,
            _: &str,
            _: &str,
            _: &[crate::ui::SelectOption],
        ) -> Option<Vec<usize>> {
            None
        }

        async fn input_with_context(&self, _: &str, _: &str, _: &str) -> Option<String> {
            Some(self.answer.clone())
        }

        async fn set_status(&self, _: &str, _: Option<&str>) {}

        async fn set_widget(&self, _: &str, _: Option<crate::ui::WidgetContent>) {}

        async fn custom(&self, _: crate::ui::ComponentSpec) -> Option<serde_json::Value> {
            None
        }
    }

    /// Each call to `stream()` pops the next response from the queue.
    struct MockProvider {
        responses: Mutex<Vec<Vec<imp_llm::Result<StreamEvent>>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|events| events.into_iter().map(Ok).collect())
                        .collect(),
                ),
            }
        }

        fn new_results(responses: Vec<Vec<imp_llm::Result<StreamEvent>>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            // We need to get the next response synchronously. Use try_lock since
            // tests are single-threaded per agent run.
            let mut responses = self.responses.try_lock().expect("MockProvider lock");
            let events = if responses.is_empty() {
                vec![Ok(StreamEvent::Error {
                    error: "No more mock responses".to_string(),
                })]
            } else {
                responses.remove(0)
            };
            let stream = futures::stream::iter(events);
            Box::pin(stream)
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("mock-key".to_string())
        }

        fn id(&self) -> &str {
            "mock"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    #[test]
    fn workflow_controller_only_overrides_soft_completion() {
        use crate::agent::workflow_integration::workflow_layer_may_override_finish;

        assert!(workflow_layer_may_override_finish(&LoopDecision::Finish {
            status: RunFinalStatus::Done {
                reason: StopReason::WorkCompleted,
            }
        }));
        assert!(workflow_layer_may_override_finish(&LoopDecision::Finish {
            status: RunFinalStatus::DoneWithConcerns {
                reason: StopReason::NoProgress,
                concerns: vec!["minor".into()],
            }
        }));
        assert!(!workflow_layer_may_override_finish(&LoopDecision::Finish {
            status: RunFinalStatus::Blocked {
                reason: StopReason::RepeatedAction,
                message: "repeated action".into(),
            }
        }));
        assert!(!workflow_layer_may_override_finish(&LoopDecision::Finish {
            status: RunFinalStatus::NeedsUserInput {
                question: "which task?".into(),
            }
        }));
        assert!(!workflow_layer_may_override_finish(
            &LoopDecision::Continue {
                reason: ContinueReason::WorkflowCloseout,
                prompt: "inspect graph".into(),
            }
        ));
    }

    #[test]
    fn workflow_closeout_does_not_override_repeated_action_finish() {
        use crate::agent::loop_policy::{DefaultLoopPolicy, LoopPolicy};
        use crate::agent::turn_assessment::{
            PostTurnAssessment, RuntimeEvidence, TextFallbackEvidence, WorkflowEvidence,
        };
        use crate::agent::workflow_integration::workflow_layer_may_override_finish;

        let assessment = PostTurnAssessment {
            runtime: RuntimeEvidence {
                repeated_action: true,
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
        };
        let decision = DefaultLoopPolicy.decide_after_turn(&assessment);

        assert!(matches!(
            decision,
            LoopDecision::Finish {
                status: RunFinalStatus::Blocked {
                    reason: StopReason::RepeatedAction,
                    ..
                }
            }
        ));
        assert!(!workflow_layer_may_override_finish(&decision));
    }

    #[tokio::test]
    async fn workflow_closeout_can_follow_soft_done() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent
            .workflow_layer
            .controller_mut()
            .record_workflow_graph_changed();

        let decision = LoopDecision::Finish {
            status: RunFinalStatus::Done {
                reason: StopReason::WorkCompleted,
            },
        };

        assert!(crate::agent::workflow_integration::workflow_layer_may_override_finish(&decision));
        assert!(matches!(
            agent.override_finish_with_workflow_decision(decision),
            LoopDecision::Continue {
                reason: ContinueReason::WorkflowCloseout,
                ..
            }
        ));
    }

    fn test_model(provider: Arc<dyn Provider>) -> Model {
        test_model_with_context_window(provider, 200_000)
    }

    fn test_model_with_context_window(provider: Arc<dyn Provider>, context_window: u32) -> Model {
        Model {
            meta: ModelMeta {
                id: "test-model".to_string(),
                provider: "mock".to_string(),
                name: "Test Model".to_string(),
                context_window,
                max_output_tokens: 16_384,
                pricing: ModelPricing {
                    input_per_mtok: 3.0,
                    output_per_mtok: 15.0,
                    cache_read_per_mtok: 0.3,
                    cache_write_per_mtok: 3.75,
                },
                capabilities: Capabilities {
                    reasoning: true,
                    images: false,
                    tool_use: true,
                },
            },
            provider,
        }
    }

    fn text_response(text: &str, input_tokens: u32, output_tokens: u32) -> Vec<StreamEvent> {
        vec![
            StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            StreamEvent::TextDelta {
                text: text.to_string(),
            },
            StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    usage: Some(Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            },
        ]
    }

    fn tool_call_response(
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Vec<StreamEvent> {
        vec![
            StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            StreamEvent::ToolCall {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments: args.clone(),
            },
            StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::ToolCall {
                        id: call_id.to_string(),
                        name: tool_name.to_string(),
                        arguments: args,
                    }],
                    usage: Some(Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::ToolUse,
                    timestamp: 1000,
                },
            },
        ]
    }

    fn multi_tool_call_response(
        calls: &[(&str, &str, serde_json::Value)],
        input_tokens: u32,
        output_tokens: u32,
    ) -> Vec<StreamEvent> {
        let mut events = vec![StreamEvent::MessageStart {
            model: "test-model".to_string(),
        }];

        let mut content = Vec::new();
        for (id, name, args) in calls {
            events.push(StreamEvent::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args.clone(),
            });
            content.push(ContentBlock::ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args.clone(),
            });
        }

        events.push(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content,
                usage: Some(Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::ToolUse,
                timestamp: 1000,
            },
        });

        events
    }

    fn make_assistant_tool_call(
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments: args,
            }],
            usage: None,
            stop_reason: LlmStopReason::ToolUse,
            timestamp: imp_llm::now(),
        })
    }

    fn make_tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
        Message::ToolResult(imp_llm::ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![ContentBlock::Text {
                text: output.to_string(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: imp_llm::now(),
        })
    }

    fn tool_result_text(message: &Message) -> Option<&str> {
        match message {
            Message::ToolResult(result) => result.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        }
    }

    /// A simple echo tool for testing.
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
                "properties": {
                    "text": { "type": "string" }
                },
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

    struct DurableWorkTool;

    #[async_trait]
    impl crate::tools::Tool for DurableWorkTool {
        fn name(&self) -> &str {
            "work"
        }

        fn label(&self) -> &str {
            "Work"
        }

        fn description(&self) -> &str {
            "Mock durable workflow tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "action": { "type": "string" } },
                "required": ["action"]
            })
        }

        fn is_readonly(&self) -> bool {
            false
        }

        async fn execute(
            &self,
            call_id: &str,
            params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            let action = params
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            Ok(crate::tools::ToolOutput {
                content: vec![ContentBlock::Text {
                    text: format!("work {action} ok"),
                }],
                details: serde_json::json!({
                    "action": action,
                    "id": format!("task-{call_id}"),
                    "item": { "id": format!("task-{call_id}"), "status": "open" }
                }),
                is_error: false,
            })
        }
    }

    struct ExtensionNetworkTool;

    #[async_trait]
    impl crate::tools::Tool for ExtensionNetworkTool {
        fn name(&self) -> &str {
            "extension_net"
        }
        fn label(&self) -> &str {
            "Extension network"
        }
        fn description(&self) -> &str {
            "Pretends to perform extension network access"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn is_readonly(&self) -> bool {
            false
        }
        fn policy_metadata(&self) -> crate::reference_monitor::ToolMetadata {
            let mut metadata = crate::reference_monitor::ToolMetadata::new(
                self.name(),
                crate::reference_monitor::ToolActionKind::Extension,
            );
            metadata.extension = true;
            metadata.extension_id = Some("test.extension".into());
            metadata.network = true;
            metadata.requires_approval = true;
            metadata
        }
        async fn execute(
            &self,
            _call_id: &str,
            _params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::text("network extension executed"))
        }
    }

    /// A mutable tool for testing write partitioning.
    #[allow(dead_code)]
    struct WriteTool;

    #[async_trait]
    impl crate::tools::Tool for WriteTool {
        fn name(&self) -> &str {
            "write"
        }
        fn label(&self) -> &str {
            "Write"
        }
        fn description(&self) -> &str {
            "Writes data"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "data": { "type": "string" }
                },
                "required": ["data"]
            })
        }
        fn is_readonly(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _call_id: &str,
            params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            let data = params["data"].as_str().unwrap_or("no data");
            Ok(crate::tools::ToolOutput::text(format!("wrote: {data}")))
        }
    }

    struct ConcurrentReadonlyState {
        readonly_expected: usize,
        readonly_started: AtomicUsize,
        readonly_finished: AtomicUsize,
        mutable_observed_finished: AtomicUsize,
        log: StdMutex<Vec<String>>,
        notify: Notify,
    }

    impl ConcurrentReadonlyState {
        fn new(readonly_expected: usize) -> Self {
            Self {
                readonly_expected,
                readonly_started: AtomicUsize::new(0),
                readonly_finished: AtomicUsize::new(0),
                mutable_observed_finished: AtomicUsize::new(0),
                log: StdMutex::new(Vec::new()),
                notify: Notify::new(),
            }
        }

        fn record(&self, entry: impl Into<String>) {
            self.log
                .lock()
                .expect("concurrent log lock")
                .push(entry.into());
        }

        async fn wait_for_all_readonly_to_start(&self) {
            while self.readonly_started.load(Ordering::SeqCst) < self.readonly_expected {
                self.notify.notified().await;
            }
        }
    }

    struct CoordinatedReadonlyTool {
        name: &'static str,
        shared: Arc<ConcurrentReadonlyState>,
    }

    #[async_trait]
    impl crate::tools::Tool for CoordinatedReadonlyTool {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "Read-only tool used to verify concurrent execution"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
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
            self.shared.record(format!("{}:start", self.name));
            self.shared.readonly_started.fetch_add(1, Ordering::SeqCst);
            self.shared.notify.notify_waiters();
            self.shared.wait_for_all_readonly_to_start().await;
            self.shared.record(format!("{}:end", self.name));
            self.shared.readonly_finished.fetch_add(1, Ordering::SeqCst);

            let text = params["text"].as_str().unwrap_or(self.name);
            Ok(crate::tools::ToolOutput::text(format!(
                "{}: {text}",
                self.name
            )))
        }
    }

    struct CoordinatedMutableTool {
        shared: Arc<ConcurrentReadonlyState>,
    }

    #[async_trait]
    impl crate::tools::Tool for CoordinatedMutableTool {
        fn name(&self) -> &str {
            "write_after_reads"
        }
        fn label(&self) -> &str {
            "Write After Reads"
        }
        fn description(&self) -> &str {
            "Mutable tool used to verify read-only tools finish first"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "data": { "type": "string" }
                },
                "required": ["data"]
            })
        }
        fn is_readonly(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _call_id: &str,
            params: serde_json::Value,
            _ctx: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            let finished = self.shared.readonly_finished.load(Ordering::SeqCst);
            self.shared
                .mutable_observed_finished
                .store(finished, Ordering::SeqCst);
            self.shared.record("write_after_reads:start");

            let data = params["data"].as_str().unwrap_or("no data");
            Ok(crate::tools::ToolOutput::text(format!(
                "wrote after reads: {data}"
            )))
        }
    }

    /// Collect all events from the handle until the channel closes.
    async fn collect_events(mut handle: AgentHandle) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = handle.event_rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn agent_queues_workflow_hint_for_planner_requests() {
        let provider = Arc::new(MockProvider::new(vec![
            text_response("Loaded workflow skill", 100, 20),
            text_response("Done", 120, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_skill_available(true);
        agent.mode = AgentMode::Planner;

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            agent
                .run("Please split this into units for workers".to_string())
                .await
                .unwrap();
        });

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts.len(), 1);
        assert_eq!(user_texts[0], "Please split this into units for workers");
    }

    #[tokio::test]
    async fn agent_queues_workflow_externalization_follow_up_after_planning_turn() {
        let provider = Arc::new(MockProvider::new(vec![
            text_response(
                "Here is the rollout decomposition: split this into phases and tasks, add dependencies, and define verification steps.",
                100,
                20,
            ),
            text_response("Externalized into workflow.", 120, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_skill_available(true);
        agent.mode = AgentMode::Planner;

        agent
            .run("Please split this rollout into tasks".to_string())
            .await
            .unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts.len(), 2);
        assert_eq!(user_texts[0], "Please split this rollout into tasks");
        assert!(user_texts[1].contains("explicitly asked for durable work structure"));
    }

    #[tokio::test]
    async fn agent_does_not_externalize_explanatory_taxonomy_answers() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "Skills are reusable playbooks. Workflows are process lanes. Subagents are delegated workers. They compose, but they are not interchangeable.",
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_skill_available(true);
        agent.mode = AgentMode::Planner;

        agent
            .run("How do workflows differ from skills or subagents?".to_string())
            .await
            .unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(
            user_texts,
            vec!["How do workflows differ from skills or subagents?".to_string()]
        );
    }

    #[test]
    fn externalization_follow_up_requires_user_externalization_intent() {
        let explanatory = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Skills are reusable playbooks. Workflows are process lanes. Subagents are delegated workers.".into(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 0,
        };
        let mut agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        agent.mode = AgentMode::Planner;
        agent.set_workflow_skill_available(true);
        assert!(!agent.should_queue_workflow_externalization_for_test(
            &explanatory,
            "How do workflows differ from skills or subagents?",
        ));

        let plan = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Here is the rollout decomposition: split this into phases and tasks, add dependencies, and define verification steps.".into(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 0,
        };
        assert!(agent.should_queue_workflow_externalization_for_test(
            &plan,
            "Please split this rollout into tasks",
        ));
    }

    #[tokio::test]
    async fn turn_assessment_debug_view_reports_execution_blocker() {
        let (agent, _handle) = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        );
        let assessment = agent.assess_post_turn(
            &AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "Verify failed.".to_string(),
                }],
                usage: None,
                stop_reason: LlmStopReason::EndTurn,
                timestamp: 0,
            },
            &[imp_llm::ToolResultMessage {
                tool_call_id: "call_verify".to_string(),
                tool_name: "workflow".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Verify failed".to_string(),
                }],
                is_error: true,
                details: serde_json::json!({
                    "action": "verify",
                    "passed": false,
                    "exit_code": 1
                }),
                timestamp: 0,
            }],
            true,
            &TurnWorkflowReview::no_change(0),
        );

        let debug = assessment.debug_view();
        assert_eq!(
            debug.runtime.execution_stop_reason.as_deref(),
            Some("execution_blocked")
        );
        assert_eq!(
            debug.chosen_action,
            NextActionDebugView::Stop {
                reason: "execution_blocked".to_string(),
            }
        );
    }

    #[test]
    fn turn_assessment_debug_view_reports_continue_recommendation() {
        let assessment = PostTurnAssessment {
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
            continue_recommendation: Some(ContinueRecommendation {
                prompt: "continue".to_string(),
                reason: ContinueReason::HighConfidenceVisibleNextStep,
            }),
        };

        let debug = assessment.debug_view();
        let recommendation = debug
            .continue_recommendation
            .expect("continue recommendation present");
        assert_eq!(recommendation.reason, "high_confidence_visible_next_step");
        assert!(matches!(
            debug.chosen_action,
            NextActionDebugView::Continue { .. }
        ));
    }

    #[tokio::test]
    async fn agent_run_artifacts_writes_trace_and_evidence_packet() {
        let temp = tempfile::TempDir::new().unwrap();
        let provider = Arc::new(MockProvider::new(vec![text_response("done", 10, 5)]));
        let model = test_model(provider);
        let (mut agent, _handle) = AgentBuilder::new(
            Config::default(),
            temp.path().to_path_buf(),
            model,
            String::new(),
        )
        .verify_command("printf verify-ok", true)
        .build()
        .unwrap();
        agent.workflow_contract_mut().autonomy_mode = crate::workflow::AutonomyMode::AllowAll;

        agent.run("Do the work".to_string()).await.unwrap();

        let runs_dir = temp.path().join(".imp").join("runs");
        let mut runs = std::fs::read_dir(&runs_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(runs.len(), 1);
        let run_dir = runs.pop().unwrap().path();
        let trace = std::fs::read_to_string(run_dir.join("trace.jsonl")).unwrap();
        assert!(trace.contains("agent.start"));
        assert!(trace.contains("agent.end"));
        let evidence = std::fs::read_to_string(run_dir.join("evidence.md")).unwrap();
        assert!(evidence.contains("# Evidence Packet"));
        assert!(evidence.contains("Do the work"));
        assert!(evidence.contains("config policy:"));
        assert!(evidence.contains("hard-rail bypass: none recorded"));
        assert!(evidence.contains("policy.checked trace events"));
        assert!(evidence.contains("trace.jsonl"));
        assert!(evidence.contains("verify-ok"));
        assert!(evidence.contains("passed"));
        assert!(run_dir.join("verification/verify-1/status.json").exists());
        assert!(run_dir.join("workflow-contract.json").exists());
    }

    #[tokio::test]
    async fn default_safe_compatibility_allows_readonly_tool() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_read",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy = first_policy_record(&events).expect("policy checked");
        assert!(policy.decision.is_allowed());
        let result = first_tool_result(&events).expect("tool end event");
        assert!(!result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("echo: hello")
        );
    }

    #[tokio::test]
    async fn default_safe_compatibility_allows_write_tool() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_write",
                "write",
                serde_json::json!({"data": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
            text_response("done", 100, 10),
            text_response("done", 100, 10),
            text_response("done", 100, 10),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(WriteTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Write hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy = first_policy_record(&events).expect("policy checked");
        assert!(policy.decision.is_allowed());
        let result = first_tool_result(&events).expect("tool end event");
        assert!(!result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("wrote: hello")
        );
    }

    #[tokio::test]
    async fn default_safe_compatibility_preserves_run_policy_tool_deny() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_echo",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));
        agent.run_policy = crate::policy::RunPolicy::new().deny_tool("echo");

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy = first_policy_record(&events).expect("policy checked");
        assert!(matches!(
            policy.decision,
            crate::reference_monitor::ToolPolicyDecision::Deny { .. }
        ));
        let result = first_tool_result(&events).expect("tool end event");
        assert!(result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("Tool `echo` denied by run policy.")
        );
    }

    #[tokio::test]
    async fn default_safe_compatibility_preserves_agent_mode_tool_deny() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_write",
                "write",
                serde_json::json!({"data": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Reviewer;
        agent.tools.register(Arc::new(WriteTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Write hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy = first_policy_record(&events).expect("policy checked");
        assert!(matches!(
            policy.decision,
            crate::reference_monitor::ToolPolicyDecision::Deny { .. }
        ));
        let result = first_tool_result(&events).expect("tool end event");
        assert!(result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("Tool 'write' is not available in reviewer mode")
        );
    }

    fn first_policy_record(
        events: &[AgentEvent],
    ) -> Option<&crate::reference_monitor::PolicyTraceRecord> {
        events.iter().find_map(|event| match event {
            AgentEvent::PolicyChecked { record } => Some(record),
            _ => None,
        })
    }

    fn first_tool_result(events: &[AgentEvent]) -> Option<&ToolResultMessage> {
        events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
            _ => None,
        })
    }

    #[tokio::test]
    async fn extension_network_policy_denial_attaches_policy_details_to_result() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response("call_ext", "extension_net", serde_json::json!({}), 100, 30),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;
        agent.tools.register(Arc::new(ExtensionNetworkTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Run extension".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy = first_policy_record(&events).expect("policy checked");
        assert_eq!(policy.tool_name, "extension_net");
        assert!(matches!(
            policy.decision,
            crate::reference_monitor::ToolPolicyDecision::Deny { .. }
        ));
        let result = first_tool_result(&events).expect("tool result");
        assert!(result.is_error);
        assert_eq!(result.details["policy"]["tool_name"], "extension_net");
        assert_eq!(
            result.details["policy"]["decision"]["reason"]["code"],
            "policy_extension_network_denied"
        );
    }

    #[tokio::test]
    async fn tool_execution_policy_routes_run_policy_deny_through_reference_monitor() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));
        agent.run_policy = crate::policy::RunPolicy::new().deny_tool("echo");

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy_event = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::PolicyChecked { record } => Some(record),
                _ => None,
            })
            .expect("policy checked event");
        assert_eq!(policy_event.tool_name, "echo");
        assert!(policy_event.args_hash.is_some());
        assert!(matches!(
            policy_event.decision,
            crate::reference_monitor::ToolPolicyDecision::Deny { .. }
        ));

        let result = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
                _ => None,
            })
            .expect("tool end event");
        assert!(result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("Tool `echo` denied by run policy.")
        );

        let checkpoint = events.iter().rev().find_map(|event| match event {
            AgentEvent::RecoveryCheckpoint { checkpoint } => checkpoint.error_class.as_deref(),
            _ => None,
        });
        assert_eq!(checkpoint, Some("run_policy_blocked"));
    }

    #[tokio::test]
    async fn tool_execution_policy_routes_agent_mode_deny_through_reference_monitor() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "write",
                serde_json::json!({"data": "hello"}),
                100,
                30,
            ),
            text_response("done", 100, 10),
        ]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Reviewer;
        agent.tools.register(Arc::new(WriteTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Write hello".to_string()).await.unwrap();
        drop(agent);
        let events = events_task.await.unwrap();

        let policy_event = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::PolicyChecked { record } => Some(record),
                _ => None,
            })
            .expect("policy checked event");
        assert_eq!(policy_event.tool_name, "write");
        assert!(matches!(
            policy_event.decision,
            crate::reference_monitor::ToolPolicyDecision::Deny { .. }
        ));

        let result = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
                _ => None,
            })
            .expect("tool end event");
        assert!(result.is_error);
        assert_eq!(
            tool_result_text(&Message::ToolResult(result.clone())),
            Some("Tool 'write' is not available in reviewer mode")
        );

        let checkpoint = events.iter().rev().find_map(|event| match event {
            AgentEvent::RecoveryCheckpoint { checkpoint } => checkpoint.error_class.as_deref(),
            _ => None,
        });
        assert_eq!(checkpoint, Some("mode_blocked"));
    }

    #[tokio::test]
    async fn emits_turn_assessment_event_for_execution_blocker() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_check",
                "bash",
                serde_json::json!({"command": "cargo check -p definitely_missing_crate", "timeout": 1}),
                100,
                20,
            ),
            text_response("The check failed.", 120, 20),
            text_response("Recovery follow-up noted.", 120, 20),
            text_response("Recovery complete.", 120, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;
        agent.tools.register(Arc::new(crate::tools::bash::BashTool));

        let events_task = tokio::spawn(collect_events(handle));
        let _ = agent.run("Run the check".to_string()).await;
        drop(agent);
        let events = events_task.await.unwrap();

        let assessment = events.iter().find_map(|event| match event {
            AgentEvent::TurnAssessment { assessment, .. } => Some(assessment),
            _ => None,
        });

        let assessment = assessment.expect("turn assessment emitted");
        assert_eq!(assessment.runtime.execution_stop_reason.as_deref(), None);
        let recommendation = assessment
            .continue_recommendation
            .as_ref()
            .expect("failed bash check should request recovery follow-up");
        assert_eq!(recommendation.reason, "execution_debt");
        assert_eq!(
            assessment.chosen_action,
            NextActionDebugView::Continue {
                prompt: failed_bash_recovery_follow_up_text().to_string(),
                reason: "execution_debt".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn failed_bash_command_with_completion_text_continues_for_recovery() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_bash",
                "bash",
                serde_json::json!({"command": "false", "timeout": 1}),
                100,
                20,
            ),
            text_response("Done.", 120, 20),
            text_response("Recovery follow-up noted.", 120, 20),
            text_response("Recovery complete.", 120, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;
        agent.tools.register(Arc::new(crate::tools::bash::BashTool));

        let _ = agent
            .run("Run the command and recover if it fails".to_string())
            .await;

        let user_follow_up = agent.messages.iter().any(|message| {
            matches!(
                message,
                Message::User(user) if user.content.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text { text } if text == failed_bash_recovery_follow_up_text()
                ))
            )
        });
        assert!(
            user_follow_up,
            "failed bash output should queue a recovery follow-up before completion text can end the run"
        );
    }

    #[test]
    fn post_turn_assessment_prefers_execution_blocker_over_completion() {
        let assessment = PostTurnAssessment {
            runtime: RuntimeEvidence {
                repeated_action: false,
                execution_stop_reason: Some(StopReason::ExecutionBlocked),
                work_completed: true,
                execution_debt: false,
                execution_evidence: false,
                planning_only_progress: false,
                orchestration_started: false,
            },
            workflow: WorkflowEvidence {
                stop_reason: Some(StopReason::DecompositionCompleted),
            },
            text_fallback: TextFallbackEvidence {
                planner_stop_reason: Some(StopReason::DecompositionCompleted),
                execution_stop_reason: Some(StopReason::WorkCompleted),
            },
            continue_recommendation: Some(ContinueRecommendation {
                prompt: "continue".to_string(),
                reason: ContinueReason::HighConfidenceVisibleNextStep,
            }),
        };

        assert_eq!(
            assessment.into_next_action(),
            NextAction::Stop {
                reason: StopReason::ExecutionBlocked,
            }
        );
    }

    #[test]
    fn post_turn_assessment_emits_continue_when_no_stop_reason_exists() {
        let assessment = PostTurnAssessment {
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
            continue_recommendation: Some(ContinueRecommendation {
                prompt: "continue".to_string(),
                reason: ContinueReason::HighConfidenceVisibleNextStep,
            }),
        };

        assert_eq!(
            assessment.into_next_action(),
            NextAction::Continue {
                prompt: "continue".to_string(),
                reason: ContinueReason::HighConfidenceVisibleNextStep,
            }
        );
    }

    #[test]
    fn execution_debt_follow_up_is_preferred_before_stopping_for_planning_only_progress() {
        let assessment = PostTurnAssessment {
            runtime: RuntimeEvidence {
                repeated_action: false,
                execution_stop_reason: None,
                work_completed: false,
                execution_debt: true,
                execution_evidence: false,
                planning_only_progress: false,
                orchestration_started: false,
            },
            workflow: WorkflowEvidence { stop_reason: None },
            text_fallback: TextFallbackEvidence {
                planner_stop_reason: None,
                execution_stop_reason: None,
            },
            continue_recommendation: Some(ContinueRecommendation {
                prompt: execution_debt_follow_up_text().to_string(),
                reason: ContinueReason::ExecutionDebt,
            }),
        };

        assert_eq!(
            assessment.into_next_action(),
            NextAction::Continue {
                prompt: execution_debt_follow_up_text().to_string(),
                reason: ContinueReason::ExecutionDebt,
            }
        );
    }

    #[test]
    fn workflow_planning_without_execution_creates_execution_debt_follow_up() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Created task".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "action": "create" }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert!(agent.workflow_execution_debt_for_test(std::slice::from_ref(&result)));
        assert!(!agent.workflow_execution_evidence_for_test(std::slice::from_ref(&result)));
        assert!(should_queue_execution_debt_follow_up(
            true, false, false, true
        ));
    }

    #[test]
    fn native_work_planning_without_execution_creates_execution_debt_follow_up() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_work".to_string(),
            tool_name: "work".to_string(),
            content: vec![ContentBlock::Text {
                text: "Created task".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "action": "create" }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert!(agent.workflow_execution_debt_for_test(std::slice::from_ref(&result)));
        assert!(!agent.workflow_execution_evidence_for_test(std::slice::from_ref(&result)));
        assert!(should_queue_execution_debt_follow_up(
            true, false, false, true
        ));
    }

    #[test]
    fn agent_can_resume_workflow_controller_from_project_run_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = crate::storage::project_run_artifacts(temp.path(), "run_resume").unwrap();
        let mut controller = crate::workflow::WorkflowRunController::new();
        controller.record_workflow_graph_changed();
        controller
            .save_to_path(&artifacts.workflow_controller_path())
            .unwrap();

        let (mut agent, _handle) = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            temp.path().to_path_buf(),
        );
        agent
            .resume_workflow_controller_from_project_run("run_resume")
            .unwrap();

        assert!(agent.workflow_layer.controller().graph_closeout_required);
    }

    #[test]
    fn workflow_run_status_result_extracts_terminal_status() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "run done".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "action": "run_state",
                "run_id": "run-42",
                "status": "done",
                "summary": { "total_failed": 0 }
            }),
            timestamp: 0,
        };

        assert_eq!(
            crate::agent::workflow_integration::workflow_run_status_from_result(&result),
            Some((
                "run-42".into(),
                crate::workflow::WorkflowChildRunStatus::Done
            ))
        );
    }

    #[test]
    fn successful_check_command_is_direct_closeout_evidence() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_bash".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text {
                text: "ok".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "command": "cargo test -p imp-core workflow::controller --lib",
                "exit_code": 0
            }),
            timestamp: 0,
        };

        assert!(bash_result_is_successful_check(&result));
    }

    #[test]
    fn workflow_run_result_extracts_run_id_for_supervision() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Started native workflow orchestration".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "action": "run", "run_id": "run-42" }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert_eq!(
            agent
                .workflow_orchestration_run_id_for_test(std::slice::from_ref(&result))
                .as_deref(),
            Some("run-42")
        );
        assert!(agent.workflow_orchestration_started_for_test(std::slice::from_ref(&result)));
    }

    #[test]
    fn native_workflow_run_result_without_child_run_id_counts_as_orchestration() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Workflow needs main agent action.".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "action": "run",
                "id": "agent-action-workflow",
                "status": "pending",
                "result": {
                    "next_action": { "kind": "agent_action" }
                }
            }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert_eq!(
            agent.workflow_orchestration_run_id_for_test(std::slice::from_ref(&result)),
            None
        );
        assert!(agent.workflow_orchestration_started_for_test(std::slice::from_ref(&result)));
    }

    #[test]
    fn workflow_run_assessment_prefers_supervision_over_work_completed() {
        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Started native workflow orchestration".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "action": "run", "run_id": "run-42" }),
            timestamp: 0,
        };
        let message = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Started run.".to_string(),
            }],
            usage: None,
            stop_reason: LlmStopReason::ToolUse,
            timestamp: 0,
        };

        let assessment = agent.assess_post_turn(
            &message,
            std::slice::from_ref(&result),
            true,
            &TurnWorkflowReview::no_change(0),
        );

        assert!(assessment.runtime.orchestration_started);
        assert_eq!(
            assessment.into_next_action(),
            NextAction::Continue {
                prompt: agent
                    .workflow_continue_recommendation(&agent.workflow_post_turn_signals(
                        std::slice::from_ref(&result),
                        &TurnWorkflowReview::no_change(0),
                    ))
                    .expect("workflow recommendation")
                    .prompt,
                reason: ContinueReason::OrchestrationProgress,
            }
        );
    }

    #[test]
    fn mutating_tool_call_satisfies_execution_evidence() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_edit".to_string(),
            tool_name: "edit".to_string(),
            content: vec![ContentBlock::Text {
                text: "diff".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "path": "src/lib.rs" }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert!(agent.workflow_execution_evidence_for_test(&[result]));
        assert!(!should_queue_execution_debt_follow_up(
            true, true, false, true
        ));
    }

    #[test]
    fn tool_results_indicate_execution_blocker_detects_failed_verify() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_verify".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Verify failed".to_string(),
            }],
            is_error: true,
            details: serde_json::json!({
                "action": "verify",
                "passed": false,
                "exit_code": 1
            }),
            timestamp: 0,
        };

        assert_eq!(
            tool_results_indicate_execution_blocker(&[result], AgentMode::Full),
            Some(StopReason::ExecutionBlocked)
        );
    }

    #[test]
    fn failed_bash_check_is_recovery_signal_not_immediate_blocker() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_bash".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text {
                text: "cargo test failed".to_string(),
            }],
            is_error: true,
            details: serde_json::json!({
                "command": "cargo test -p imp-core",
                "exit_code": 101
            }),
            timestamp: 0,
        };

        assert_eq!(
            tool_results_indicate_execution_blocker(std::slice::from_ref(&result), AgentMode::Full),
            None
        );
        assert!(tool_results_indicate_failed_bash_command(
            &[result],
            AgentMode::Full
        ));
    }

    #[test]
    fn failed_bash_after_successful_edit_is_recovery_signal_not_runtime_blocker() {
        let edit_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_edit".to_string(),
            tool_name: "edit".to_string(),
            content: vec![ContentBlock::Text {
                text: "edited file".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "path": "src/lib.rs" }),
            timestamp: 0,
        };
        let bash_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_bash".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text {
                text: "compiler error".to_string(),
            }],
            is_error: true,
            details: serde_json::json!({
                "command": "cargo check -p imp-core",
                "exit_code": 101
            }),
            timestamp: 0,
        };

        assert_eq!(
            tool_results_indicate_execution_blocker(
                &[edit_result, bash_result.clone()],
                AgentMode::Full
            ),
            None
        );
        assert!(tool_results_indicate_failed_bash_command(
            &[bash_result],
            AgentMode::Full
        ));
    }

    #[test]
    fn records_and_resolves_edited_files_verification_obligation() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;

        let edit_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_edit".to_string(),
            tool_name: "edit".to_string(),
            content: vec![ContentBlock::Text {
                text: "edited file".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({ "path": "src/lib.rs" }),
            timestamp: 0,
        };
        agent.record_obligations_from_tool_results(&[edit_result]);

        assert!(agent
            .obligation_ledger
            .contains(autonomy::ObligationKind::EditedFilesVerification));
        let (prompt, reason) = agent
            .obligation_ledger
            .next_continue()
            .expect("edit should create verification obligation");
        assert_eq!(reason, ContinueReason::ExecutionDebt);
        assert!(prompt.contains("run the narrowest relevant verification"));

        let check_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_check".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text {
                text: "ok".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "exit_code": 0,
                "command": "cargo check -p imp-core"
            }),
            timestamp: 0,
        };
        agent.record_obligations_from_tool_results(&[check_result]);

        assert!(!agent
            .obligation_ledger
            .contains(autonomy::ObligationKind::EditedFilesVerification));
    }

    #[tokio::test]
    async fn ask_user_answer_is_sent_back_to_model() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_ask",
                "ask_user",
                serde_json::json!({"question": "Which color?"}),
                100,
                20,
            ),
            text_response("You chose blue.", 120, 15),
        ]));
        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(crate::tools::ask::AskTool));
        agent.ui = Arc::new(AnsweringUi {
            answer: "blue".to_string(),
        });

        agent.run("Ask me a color".to_string()).await.unwrap();

        assert!(agent.messages.iter().any(|message| {
            matches!(
                message,
                Message::ToolResult(result)
                    if result.tool_name == "ask_user"
                        && result
                            .content
                            .iter()
                            .any(|block| matches!(block, ContentBlock::Text { text } if text == "blue"))
            )
        }));
        assert!(agent.messages.iter().any(|message| {
            matches!(
                message,
                Message::Assistant(assistant)
                    if assistant_message_text(assistant).contains("You chose blue.")
            )
        }));
    }

    #[test]
    fn tool_results_indicate_execution_blocker_does_not_stop_on_ask_tool_answer() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_ask".to_string(),
            tool_name: "ask_user".to_string(),
            content: vec![ContentBlock::Text {
                text: "blue".to_string(),
            }],
            is_error: false,
            details: serde_json::Value::Null,
            timestamp: 0,
        };

        assert_eq!(
            tool_results_indicate_execution_blocker(&[result], AgentMode::Full),
            None
        );
    }

    #[test]
    fn workflow_close_is_workflow_progress_not_runtime_completion() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_workflow".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Closed task".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "action": "close",
                "unit": { "id": "1", "status": "closed" }
            }),
            timestamp: 0,
        };

        assert!(!tool_results_indicate_work_completed(
            std::slice::from_ref(&result),
            AgentMode::Full
        ));
        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert!(agent.workflow_durable_progress_for_test(&[result]));
    }

    #[test]
    fn tool_results_indicate_work_completed_detects_edit_plus_successful_check() {
        let edit_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_edit".to_string(),
            tool_name: "edit".to_string(),
            content: vec![ContentBlock::Text {
                text: "diff output".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "path": "/tmp/file.rs"
            }),
            timestamp: 0,
        };
        let check_result = imp_llm::ToolResultMessage {
            tool_call_id: "call_check".to_string(),
            tool_name: "bash".to_string(),
            content: vec![ContentBlock::Text {
                text: "ok".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "exit_code": 0,
                "command": "cargo check -p imp-core"
            }),
            timestamp: 0,
        };

        assert!(tool_results_indicate_work_completed(
            &[edit_result, check_result],
            AgentMode::Full
        ));
    }

    #[test]
    fn tool_results_indicate_work_completed_detects_closed_unit_details() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_close".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Closed unit 1".to_string(),
            }],
            is_error: false,
            details: serde_json::json!({
                "action": "close",
                "unit": {
                    "id": "1",
                    "title": "Test unit",
                    "status": "closed"
                }
            }),
            timestamp: 0,
        };

        let agent = Agent::new(
            test_model(Arc::new(MockProvider::new(vec![]))),
            PathBuf::from("/tmp"),
        )
        .0;
        assert!(agent.workflow_durable_progress_for_test(&[result]));
    }

    #[test]
    fn workflow_review_needs_decision_maps_to_user_blocker() {
        let review = TurnWorkflowReview {
            turn_index: 0,
            state: crate::workflow_review::WorkflowReviewState::NeedsDecision,
            scope: crate::workflow_review::WorkflowReviewScope::default(),
            anchor_unit: None,
            touched_units: Vec::new(),
            proposed_children: Vec::new(),
            material_field_changes: Vec::new(),
            notes_appended: Vec::new(),
            decision_events: Vec::new(),
            unresolved_consequential_choices: Vec::new(),
            next_question: Some("Which path should we take?".to_string()),
        };

        assert_eq!(
            {
                let mut agent = Agent::new(
                    test_model(Arc::new(MockProvider::new(vec![]))),
                    PathBuf::from("/tmp"),
                )
                .0;
                agent.mode = AgentMode::Planner;
                agent.workflow_review_stop_reason_for_test(&review)
            },
            Some(StopReason::UserBlocker)
        );
    }

    #[test]
    fn workflow_review_changed_with_planner_children_maps_to_decomposition_completed() {
        let review = TurnWorkflowReview {
            turn_index: 0,
            state: crate::workflow_review::WorkflowReviewState::Changed,
            scope: crate::workflow_review::WorkflowReviewScope::default(),
            anchor_unit: None,
            touched_units: Vec::new(),
            proposed_children: vec![crate::workflow_review::TurnWorkflowProposedChild {
                unit: crate::workflow_review::WorkflowUnitRef::new(
                    "28.6.1",
                    "child",
                    Some("job".to_string()),
                ),
                parent: crate::workflow_review::WorkflowUnitRef::new(
                    "28.6",
                    "parent",
                    Some("epic".to_string()),
                ),
                child_kind: crate::workflow_review::WorkflowReviewUnitKind::Job,
                child_origin: crate::workflow_review::WorkflowUnitOrigin::CreatedInTurn,
            }],
            material_field_changes: Vec::new(),
            notes_appended: Vec::new(),
            decision_events: Vec::new(),
            unresolved_consequential_choices: Vec::new(),
            next_question: None,
        };

        assert_eq!(
            {
                let mut agent = Agent::new(
                    test_model(Arc::new(MockProvider::new(vec![]))),
                    PathBuf::from("/tmp"),
                )
                .0;
                agent.mode = AgentMode::Planner;
                agent.workflow_review_stop_reason_for_test(&review)
            },
            Some(StopReason::DecompositionCompleted)
        );
    }

    #[tokio::test]
    async fn planner_stops_after_decomposition_is_externalized() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "Externalized into workflow. Plan is complete and ready for handoff.",
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Planner;
        agent.set_workflow_skill_available(true);

        agent.run("Plan the rollout".to_string()).await.unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts, vec!["Plan the rollout".to_string()]);
    }

    #[tokio::test]
    async fn planner_stops_for_user_blocker_instead_of_auto_follow_up() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "Blocked: I need your input on which auth direction we should choose before continuing.",
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Planner;
        agent.set_workflow_skill_available(true);

        agent.run("Plan the rollout".to_string()).await.unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts, vec!["Plan the rollout".to_string()]);
    }

    #[tokio::test]
    async fn native_work_planning_then_done_continues_to_execution_debt_follow_up() {
        let provider = Arc::new(MockProvider::new(vec![
            multi_tool_call_response(
                &[
                    (
                        "call_work_1",
                        "work",
                        serde_json::json!({"action": "create", "kind": "task", "title": "Plan"}),
                    ),
                    (
                        "call_work_2",
                        "work",
                        serde_json::json!({"action": "update", "id": "task-1", "summary": "planned"}),
                    ),
                    (
                        "call_work_3",
                        "work",
                        serde_json::json!({"action": "claim", "id": "task-1"}),
                    ),
                    (
                        "call_work_4",
                        "work",
                        serde_json::json!({"action": "context", "id": "task-1"}),
                    ),
                ],
                100,
                30,
            ),
            text_response("Done.", 120, 5),
            text_response("Done.", 130, 5),
            text_response("Done.", 140, 5),
            text_response("Done.", 150, 5),
            text_response("Done.", 160, 5),
            text_response("Done.", 170, 5),
            text_response("Done.", 180, 5),
            text_response("Done.", 190, 5),
            text_response("Done.", 200, 5),
        ]));
        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;
        agent.tools.register(Arc::new(DurableWorkTool));

        let _ = agent.run("Implement the runtime fix".to_string()).await;

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert!(
            user_texts.iter().any(|text| {
                text.contains("You have recorded or planned work")
                    || text.contains("Workflow state changed")
                    || text.contains("Workflow graph state changed")
            }),
            "expected execution-debt follow-up after workflow planning, got {user_texts:?}"
        );
    }

    #[tokio::test]
    async fn agent_queues_workflow_basics_hint_for_worker_workflow_requests() {
        let provider = Arc::new(MockProvider::new(vec![
            text_response("Loaded basics skill", 100, 20),
            text_response("Done", 120, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_basics_skill_available(true);
        agent.mode = AgentMode::Worker;

        agent
            .run("Check workflow status and logs for my unit".to_string())
            .await
            .unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts.len(), 1);
        assert_eq!(user_texts[0], "Check workflow status and logs for my unit");
    }

    #[tokio::test]
    async fn agent_does_not_queue_workflow_hint_without_matching_signal() {
        let provider = Arc::new(MockProvider::new(vec![text_response("No nudge", 100, 20)]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_skill_available(true);
        agent.mode = AgentMode::Planner;

        agent
            .run("Explain how this parser works".to_string())
            .await
            .unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(
            user_texts,
            vec!["Explain how this parser works".to_string()]
        );
    }

    #[tokio::test]
    async fn agent_does_not_queue_workflow_basics_hint_when_no_tools_available() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "Loaded basics skill",
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.set_workflow_basics_skill_available(true);
        agent.mode = AgentMode::Worker;
        agent.tools.retain(|_| false);

        agent
            .run("Check workflow status and logs for my unit".to_string())
            .await
            .unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(
            user_texts,
            vec!["Check workflow status and logs for my unit".to_string()]
        );
    }

    #[tokio::test]
    async fn single_text_turn_with_no_tools_exits_cleanly() {
        let provider = Arc::new(MockProvider::new(vec![text_response("SMOKE_OK", 50, 10)]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Worker;
        agent.set_workflow_basics_skill_available(true);
        agent.tools.retain(|_| false);

        let events_task = tokio::spawn(collect_events(handle));
        let result = agent
            .run("Check workflow status and finish".to_string())
            .await;
        drop(agent);

        assert!(result.is_ok());

        let events = events_task.await.unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
        assert!(!events.iter().any(|e| matches!(
            e,
            AgentEvent::Error { error } if error.contains("Max turns exceeded")
        )));
    }

    #[tokio::test]
    async fn agent_emits_timing_events_in_order() {
        let provider = Arc::new(MockProvider::new(vec![text_response("timed", 10, 5)]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("time this".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let timings: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Timing { timing } => Some(timing.clone()),
                _ => None,
            })
            .collect();

        assert!(timings.len() >= 7);
        assert_eq!(timings[0].stage, TimingStage::ContextAssemblyStart);
        assert_eq!(timings[1].stage, TimingStage::ContextAssemblyEnd);
        assert_eq!(timings[2].stage, TimingStage::LlmRequestStart);
        assert_eq!(timings[3].stage, TimingStage::FirstStreamEvent);
        assert_eq!(timings[4].stage, TimingStage::FirstTextDelta);
        assert!(timings
            .iter()
            .any(|timing| timing.stage == TimingStage::MessageEnd));
        assert!(timings
            .iter()
            .any(|timing| timing.stage == TimingStage::PostTurnAssessmentEnd));

        for timing in timings {
            assert_eq!(timing.turn, 0);
            if let Some(since_llm_request_start_ms) = timing.since_llm_request_start_ms {
                assert!(timing.since_turn_start_ms >= since_llm_request_start_ms);
            }
        }
    }

    #[tokio::test]
    async fn agent_streams_message_delta_before_message_end() {
        let provider = Arc::new(MockProvider::new_results(vec![vec![
            Ok(StreamEvent::MessageStart {
                model: "test-model".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "streaming".to_string(),
            }),
            Ok(StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "streaming".to_string(),
                    }],
                    usage: Some(Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            }),
        ]]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Say hi".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let text_delta_idx = events.iter().position(|event| {
            matches!(
                event,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "streaming"
            )
        });
        let turn_end_idx = events
            .iter()
            .position(|event| matches!(event, AgentEvent::TurnEnd { .. }));

        assert!(text_delta_idx.is_some());
        assert!(turn_end_idx.is_some());
        assert!(text_delta_idx.unwrap() < turn_end_idx.unwrap());
    }

    #[tokio::test]
    async fn agent_treats_provider_terminal_error_as_failure_not_blank_turn() {
        let provider = Arc::new(MockProvider::new_results(vec![vec![
            Ok(StreamEvent::MessageStart {
                model: "test-model".to_string(),
            }),
            Ok(StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![],
                    usage: Some(Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::Error(
                        "context_length_exceeded: input too large".to_string(),
                    ),
                    timestamp: 1000,
                },
            }),
        ]]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        let result = agent.run("Continue".to_string()).await;
        drop(agent);

        assert!(result.is_err());
        let events = events_task.await.unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Error { error }
                if error == "context_length_exceeded: input too large"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::AgentEnd {
                status: RunFinalStatus::Failed { message },
                ..
            } if message == "context_length_exceeded: input too large"
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::TurnEnd { .. })));
    }

    #[tokio::test]
    async fn agent_retries_before_first_meaningful_event_but_not_after() {
        let provider = Arc::new(MockProvider::new_results(vec![
            vec![
                Ok(StreamEvent::MessageStart {
                    model: "test-model".to_string(),
                }),
                Err(imp_llm::Error::Stream("startup failure".into())),
            ],
            vec![
                Ok(StreamEvent::MessageStart {
                    model: "test-model".to_string(),
                }),
                Ok(StreamEvent::TextDelta {
                    text: "recovered".to_string(),
                }),
                Ok(StreamEvent::MessageEnd {
                    message: AssistantMessage {
                        content: vec![ContentBlock::Text {
                            text: "recovered".to_string(),
                        }],
                        usage: Some(Usage {
                            input_tokens: 10,
                            output_tokens: 5,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                        }),
                        stop_reason: LlmStopReason::EndTurn,
                        timestamp: 1000,
                    },
                }),
            ],
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Recover".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let text_delta = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "recovered"
            )
        });
        let turn_end = events
            .iter()
            .position(|e| matches!(e, AgentEvent::TurnEnd { .. }));

        assert!(text_delta.is_some());
        assert!(turn_end.is_some());
        assert!(text_delta.unwrap() < turn_end.unwrap());
    }

    #[tokio::test]
    async fn agent_recovers_after_partial_stream_failure() {
        let provider = Arc::new(MockProvider::new_results(vec![
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "partial".to_string(),
                }),
                Err(imp_llm::Error::Stream("mid-stream failure".into())),
            ],
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "recovered".to_string(),
                }),
                Ok(StreamEvent::MessageEnd {
                    message: AssistantMessage {
                        content: vec![ContentBlock::Text {
                            text: "recovered".to_string(),
                        }],
                        usage: Some(Usage::default()),
                        stop_reason: LlmStopReason::EndTurn,
                        timestamp: 1000,
                    },
                }),
            ],
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent
            .run("Recover after partial stream failure".to_string())
            .await
            .unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let error_idx = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::Error { error }
                if error.contains("Provider stream failed after partial output")
                    && error.contains("mid-stream failure")
            )
        });
        let recovered_idx = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "recovered"
            )
        });
        let failed_end = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::AgentEnd {
                    status: RunFinalStatus::Failed { .. },
                    ..
                }
            )
        });

        assert!(error_idx.is_some());
        assert!(recovered_idx.is_some());
        assert!(error_idx.unwrap() < recovered_idx.unwrap());
        assert!(!failed_end);
    }

    #[tokio::test]
    async fn agent_surfaces_error_after_repeated_partial_stream_failures() {
        let provider = Arc::new(MockProvider::new_results(vec![
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "partial-1".to_string(),
                }),
                Err(imp_llm::Error::Stream("mid-stream failure 1".into())),
            ],
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "partial-2".to_string(),
                }),
                Err(imp_llm::Error::Stream("mid-stream failure 2".into())),
            ],
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "partial-3".to_string(),
                }),
                Err(imp_llm::Error::Stream("mid-stream failure 3".into())),
            ],
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        let result = agent
            .run("Fail after repeated stream recovery failures".to_string())
            .await;
        drop(agent);

        assert!(result.is_err());

        let events = events_task.await.unwrap();
        let text_delta = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "partial-1"
            )
        });
        let error_idx = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::Error { error }
                if error.contains("Provider stream failed after partial output")
                    && error.contains("mid-stream failure")
            )
        });

        assert!(text_delta.is_some());
        assert!(error_idx.is_some());
        assert!(text_delta.unwrap() < error_idx.unwrap());
    }

    #[tokio::test]
    async fn agent_recovers_after_silent_eof_without_message_end() {
        let provider = Arc::new(MockProvider::new_results(vec![
            vec![Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            })],
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "recovered".to_string(),
                }),
                Ok(StreamEvent::MessageEnd {
                    message: AssistantMessage {
                        content: vec![ContentBlock::Text {
                            text: "recovered".to_string(),
                        }],
                        usage: Some(Usage::default()),
                        stop_reason: LlmStopReason::EndTurn,
                        timestamp: 1000,
                    },
                }),
            ],
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent
            .run("Recover from silent eof".to_string())
            .await
            .unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let text_delta = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "partial"
            )
        });
        let error_idx = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::Error { error }
                if error.contains("missing terminal completion event")
            )
        });
        let recovered_idx = events.iter().position(|e| {
            matches!(
                e,
                AgentEvent::MessageDelta {
                    delta: StreamEvent::TextDelta { text }
                } if text == "recovered"
            )
        });

        assert!(text_delta.is_some());
        assert!(error_idx.is_some());
        assert!(recovered_idx.is_some());
        assert!(text_delta.unwrap() < error_idx.unwrap());
        assert!(error_idx.unwrap() < recovered_idx.unwrap());
    }

    // ── Test 1: Simple text response ───────────────────────────────

    #[tokio::test]
    async fn agent_simple_text_response() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "Hello, world!",
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Say hello".to_string()).await.unwrap();
        drop(agent); // close event channel

        let events = events_task.await.unwrap();

        // Verify event order: AgentStart → TurnStart → deltas → TurnEnd → AgentEnd
        assert!(matches!(events[0], AgentEvent::AgentStart { .. }));

        let turn_start = events
            .iter()
            .position(|e| matches!(e, AgentEvent::TurnStart { index: 0 }));
        assert!(turn_start.is_some());

        let turn_end = events
            .iter()
            .position(|e| matches!(e, AgentEvent::TurnEnd { index: 0, .. }));
        assert!(turn_end.is_some());
        assert!(turn_end.unwrap() > turn_start.unwrap());

        let agent_end = events
            .iter()
            .position(|e| matches!(e, AgentEvent::AgentEnd { .. }));
        assert!(agent_end.is_some());
        assert!(agent_end.unwrap() > turn_end.unwrap());

        // Verify usage
        if let AgentEvent::AgentEnd { usage, cost, .. } = &events[agent_end.unwrap()] {
            assert_eq!(usage.input_tokens, 100);
            assert_eq!(usage.output_tokens, 20);
            assert!(cost.total > 0.0);
        } else {
            panic!("Expected AgentEnd");
        }

        // Only one turn (no tool calls)
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert_eq!(turn_starts.len(), 1);
    }

    // ── Test 2: Single tool call → result → text response ──────────

    #[tokio::test]
    async fn agent_single_tool_call() {
        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: model calls echo tool
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                30,
            ),
            // Turn 1: model responds with text after seeing tool result
            text_response("The echo said: hello", 200, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo hello".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // Should have 2 TurnStart events (turn 0 with tool, turn 1 with text)
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert_eq!(turn_starts.len(), 2);

        // Should have tool execution events
        let tool_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
            .collect();
        assert_eq!(tool_starts.len(), 1);

        let tool_ends: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
            .collect();
        assert_eq!(tool_ends.len(), 1);

        // Verify accumulated usage across turns (100 + 200 input, 30 + 25 output)
        if let Some(AgentEvent::AgentEnd { usage, .. }) = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        {
            assert_eq!(usage.input_tokens, 300);
            assert_eq!(usage.output_tokens, 55);
        } else {
            panic!("Expected AgentEnd");
        }
    }

    // ── Test 3: Multiple tool calls → follow-up tool calls → done ──

    #[tokio::test]
    async fn agent_multiple_tool_calls() {
        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: model calls echo twice
            multi_tool_call_response(
                &[
                    ("call_1", "echo", serde_json::json!({"text": "first"})),
                    ("call_2", "echo", serde_json::json!({"text": "second"})),
                ],
                100,
                40,
            ),
            // Turn 1: model calls echo once more
            tool_call_response(
                "call_3",
                "echo",
                serde_json::json!({"text": "third"}),
                200,
                20,
            ),
            // Turn 2: model responds with final text
            text_response("All done!", 300, 10),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Echo three things".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // 3 turns
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert_eq!(turn_starts.len(), 3);

        // 3 tool executions total
        let tool_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
            .collect();
        assert_eq!(tool_starts.len(), 3);

        // Total usage: 100+200+300=600 input, 40+20+10=70 output
        if let Some(AgentEvent::AgentEnd { usage, .. }) = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        {
            assert_eq!(usage.input_tokens, 600);
            assert_eq!(usage.output_tokens, 70);
        } else {
            panic!("Expected AgentEnd");
        }
    }

    // ── Test 4: Cancel command mid-run ─────────────────────────────

    #[tokio::test]
    async fn execution_does_not_stop_after_work_completed_text() {
        let provider = Arc::new(MockProvider::new(vec![
            text_response(
                "All done! Implemented the change and finished the task.",
                100,
                20,
            ),
            text_response("No further runtime evidence is available.", 100, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;

        agent.run("Implement the change".to_string()).await.unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts, vec!["Implement the change".to_string()]);
        assert_eq!(
            agent
                .assess_post_turn(
                    agent
                        .messages
                        .iter()
                        .rev()
                        .find_map(|message| match message {
                            Message::Assistant(assistant) => Some(assistant),
                            _ => None,
                        })
                        .unwrap(),
                    &[],
                    false,
                    &TurnWorkflowReview::no_change(0),
                )
                .text_fallback
                .execution_stop_reason,
            None
        );
    }

    #[tokio::test]
    async fn execution_does_not_stop_for_user_blocker_text() {
        let provider = Arc::new(MockProvider::new(vec![
            text_response(
                "Blocked: I need your input on which path to take before continuing.",
                100,
                20,
            ),
            text_response("No further runtime evidence is available.", 100, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.mode = AgentMode::Full;

        agent.run("Implement the change".to_string()).await.unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(user_texts, vec!["Implement the change".to_string()]);
        assert_eq!(
            agent
                .assess_post_turn(
                    agent
                        .messages
                        .iter()
                        .rev()
                        .find_map(|message| match message {
                            Message::Assistant(assistant) => Some(assistant),
                            _ => None,
                        })
                        .unwrap(),
                    &[],
                    false,
                    &TurnWorkflowReview::no_change(0),
                )
                .text_fallback
                .execution_stop_reason,
            None
        );
    }

    #[tokio::test]
    async fn agent_follow_up_runs_after_current_work_finishes() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                20,
            ),
            text_response("Handled follow-up", 120, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        handle
            .command_tx
            .send(AgentCommand::FollowUp("What next?".into()))
            .await
            .unwrap();

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Do the first thing".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert_eq!(turn_starts.len(), 2);
    }

    #[tokio::test]
    async fn agent_follow_up_preserves_order_with_multiple_messages() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                20,
            ),
            text_response("First follow-up handled", 120, 25),
            text_response("Second follow-up handled", 130, 30),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        handle
            .command_tx
            .send(AgentCommand::FollowUp("follow up one".into()))
            .await
            .unwrap();
        handle
            .command_tx
            .send(AgentCommand::FollowUp("follow up two".into()))
            .await
            .unwrap();

        agent.run("Do the first thing".to_string()).await.unwrap();

        let user_texts: Vec<String> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => user.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();

        assert_eq!(
            user_texts,
            vec![
                "Do the first thing".to_string(),
                "follow up one".to_string(),
                "follow up two".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn agent_cancel_still_wins_over_follow_up_queue() {
        let provider = Arc::new(MockProvider::new(vec![tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            20,
        )]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        handle
            .command_tx
            .send(AgentCommand::FollowUp("queued later".into()))
            .await
            .unwrap();
        handle.command_tx.send(AgentCommand::Cancel).await.unwrap();

        let result = agent.run("Do something".to_string()).await;
        assert!(matches!(result, Err(crate::error::Error::Cancelled)));
    }

    #[tokio::test]
    async fn agent_allows_non_workflow_bash_commands() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "bash",
                serde_json::json!({"command": "printf 'ok'", "timeout": 5}),
                100,
                20,
            ),
            text_response("done", 120, 25),
        ]));

        let model = test_model(provider);
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(crate::tools::bash::BashTool));

        agent.run("Run a shell command".to_string()).await.unwrap();

        let tool_result = agent
            .messages
            .iter()
            .find_map(|message| match message {
                Message::ToolResult(result) => Some(result),
                _ => None,
            })
            .expect("expected tool result");
        assert!(!tool_result.is_error);
    }

    #[tokio::test]
    async fn agent_cancel_mid_run() {
        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: tool call (agent will process this, then see Cancel before turn 1)
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                100,
                20,
            ),
            // Turn 1: this should never be reached
            text_response("Should not see this", 100, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        // Send cancel before the second turn
        handle.command_tx.send(AgentCommand::Cancel).await.unwrap();

        let events_task = tokio::spawn(collect_events(handle));
        let result = agent.run("Do something".to_string()).await;
        drop(agent);

        // Should return Cancelled error
        assert!(matches!(result, Err(crate::error::Error::Cancelled)));

        let events = events_task.await.unwrap();

        // Should have AgentEnd
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));

        // Should NOT have a second turn
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert!(turn_starts.len() <= 1);
    }

    #[tokio::test]
    async fn single_text_turn_exits_cleanly() {
        let provider = Arc::new(MockProvider::new(vec![text_response("SMOKE_OK", 50, 10)]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

        let events_task = tokio::spawn(collect_events(handle));
        let result = agent.run("Reply once and stop".to_string()).await;
        drop(agent);

        assert!(result.is_ok());

        let events = events_task.await.unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
        assert!(!events.iter().any(|e| matches!(
            e,
            AgentEvent::Error { error } if error.contains("Max turns exceeded")
        )));
    }

    // ── Test 6: Unknown tool → error result → model self-corrects ──

    #[tokio::test]
    async fn agent_unknown_tool_self_corrects() {
        let provider = Arc::new(MockProvider::new(vec![
            // Turn 0: model calls a tool that doesn't exist
            tool_call_response(
                "call_1",
                "nonexistent",
                serde_json::json!({"foo": "bar"}),
                100,
                20,
            ),
            // Turn 1: model self-corrects and responds with text
            text_response("Sorry, I used the wrong tool. Here's the answer.", 200, 30),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        // Deliberately NOT registering the "nonexistent" tool

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Do something".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // The tool execution should produce an error result
        let tool_end = events
            .iter()
            .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
        assert!(tool_end.is_some());
        if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
            assert!(result.is_error);
            let text = result.content.iter().find_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            });
            assert!(text.unwrap().contains("Unknown tool"));
        }

        // Model should have self-corrected in turn 1
        let turn_starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
            .collect();
        assert_eq!(turn_starts.len(), 2);

        // Should complete successfully
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
    }

    #[tokio::test]
    async fn agent_concurrent_readonly() {
        let shared = Arc::new(ConcurrentReadonlyState::new(3));
        let provider = Arc::new(MockProvider::new(vec![
            multi_tool_call_response(
                &[
                    ("call_ro_1", "echo_a", serde_json::json!({"text": "first"})),
                    (
                        "call_write",
                        "write_after_reads",
                        serde_json::json!({"data": "mutate"}),
                    ),
                    ("call_ro_2", "echo_b", serde_json::json!({"text": "second"})),
                    ("call_ro_3", "echo_c", serde_json::json!({"text": "third"})),
                ],
                100,
                40,
            ),
            text_response("All tools finished", 150, 20),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        drop(handle);

        agent.tools.register(Arc::new(CoordinatedReadonlyTool {
            name: "echo_a",
            shared: shared.clone(),
        }));
        agent.tools.register(Arc::new(CoordinatedReadonlyTool {
            name: "echo_b",
            shared: shared.clone(),
        }));
        agent.tools.register(Arc::new(CoordinatedReadonlyTool {
            name: "echo_c",
            shared: shared.clone(),
        }));
        agent.tools.register(Arc::new(CoordinatedMutableTool {
            shared: shared.clone(),
        }));

        tokio::time::timeout(
            Duration::from_millis(250),
            agent.run("Run all tools".to_string()),
        )
        .await
        .expect("read-only tools should not block each other")
        .expect("agent should complete successfully");

        let tool_result_ids: Vec<_> = agent
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_result_ids,
            vec!["call_ro_1", "call_write", "call_ro_2", "call_ro_3"]
        );

        assert_eq!(shared.readonly_started.load(Ordering::SeqCst), 3);
        assert_eq!(shared.readonly_finished.load(Ordering::SeqCst), 3);
        assert_eq!(shared.mutable_observed_finished.load(Ordering::SeqCst), 3);

        let log = shared.log.lock().expect("concurrent log lock").clone();
        assert_eq!(
            log.last().map(String::as_str),
            Some("write_after_reads:start")
        );
    }

    // ── Event ordering validation ──────────────────────────────────

    #[tokio::test]
    async fn agent_event_ordering() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "hello"}),
                50,
                10,
            ),
            text_response("Done", 50, 10),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("test".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // Extract event types in order
        let types: Vec<&str> = events
            .iter()
            .map(|e| match e {
                AgentEvent::AgentStart { .. } => "AgentStart",
                AgentEvent::AgentEnd { .. } => "AgentEnd",
                AgentEvent::TurnStart { .. } => "TurnStart",
                AgentEvent::TurnEnd { .. } => "TurnEnd",
                AgentEvent::MessageDelta { .. } => "MessageDelta",
                AgentEvent::ToolExecutionStart { .. } => "ToolExecStart",
                AgentEvent::ToolExecutionEnd { .. } => "ToolExecEnd",
                AgentEvent::Warning { .. } => "Warning",
                AgentEvent::EvidenceWritten { .. } => "EvidenceWritten",
                AgentEvent::VerificationStarted { .. } => "VerificationStarted",
                AgentEvent::VerificationCompleted { .. } => "VerificationCompleted",
                AgentEvent::PolicyChecked { .. } => "PolicyChecked",
                AgentEvent::Error { .. } => "Error",
                _ => "Other",
            })
            .collect();

        // Must start with AgentStart
        assert_eq!(types[0], "AgentStart");

        // Must end with AgentEnd
        assert_eq!(types[types.len() - 1], "AgentEnd");

        // TurnStart must come before TurnEnd for each turn
        let mut turn_start_indices: Vec<usize> = Vec::new();
        let mut turn_end_indices: Vec<usize> = Vec::new();
        for (i, t) in types.iter().enumerate() {
            if *t == "TurnStart" {
                turn_start_indices.push(i);
            }
            if *t == "TurnEnd" {
                turn_end_indices.push(i);
            }
        }
        assert_eq!(turn_start_indices.len(), 2);
        assert_eq!(turn_end_indices.len(), 2);
        for i in 0..turn_start_indices.len() {
            assert!(turn_start_indices[i] < turn_end_indices[i]);
        }

        // ToolExecStart must come before ToolExecEnd
        let tool_start = types.iter().position(|t| *t == "ToolExecStart");
        let tool_end = types.iter().position(|t| *t == "ToolExecEnd");
        assert!(tool_start.is_some());
        assert!(tool_end.is_some());
        assert!(tool_start.unwrap() < tool_end.unwrap());
    }

    #[tokio::test]
    async fn agent_fires_hooks() {
        let provider = Arc::new(MockProvider::new(vec![text_response("hooked", 100, 20)]));
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        drop(handle);

        let hook_calls = Arc::new(AtomicUsize::new(0));
        let hook_calls_for_callback = hook_calls.clone();
        agent.hooks.register(crate::hooks::HookDefinition {
            event: "before_llm_call".to_string(),
            match_pattern: None,
            action: crate::hooks::HookAction::Callback(Arc::new(move |_event| {
                hook_calls_for_callback.fetch_add(1, Ordering::SeqCst);
                crate::hooks::HookResult::default()
            })),
            blocking: true,
            threshold: None,
        });

        agent.run("Run once".to_string()).await.unwrap();

        assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn agent_context_masking() {
        let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));

        let mut seeded_messages = Vec::new();
        for index in 0..12 {
            let call_id = format!("call_{index}");
            seeded_messages.push(make_assistant_tool_call(
                &call_id,
                "read",
                serde_json::json!({"path": format!("src/file_{index}.rs")}),
            ));
            seeded_messages.push(make_tool_result(&call_id, "read", &"x".repeat(400)));
        }

        let mut usage_messages = seeded_messages.clone();
        usage_messages.push(Message::user("trigger masking"));
        let provisional_model = test_model(provider.clone());
        let usage = crate::context::context_usage(&usage_messages, &provisional_model);
        let context_window = ((usage.used as f64) / 0.7).ceil() as u32;

        let model = test_model_with_context_window(provider, context_window.max(1));
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        drop(handle);
        agent.messages = seeded_messages;

        agent.run("trigger masking".to_string()).await.unwrap();

        let masked = tool_result_text(&agent.messages[1]).expect("first tool result text");
        assert!(masked.starts_with("[Output omitted"));

        let recent_index = (10 * 2) + 1;
        let recent =
            tool_result_text(&agent.messages[recent_index]).expect("recent tool result text");
        let expected_recent = "x".repeat(400);
        assert_eq!(recent, expected_recent.as_str());
    }

    #[tokio::test]
    async fn agent_masks_observations_when_context_is_tight() {
        let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));

        let mut seeded_messages = Vec::new();
        for index in 0..12 {
            let call_id = format!("call_{index}");
            seeded_messages.push(make_assistant_tool_call(
                &call_id,
                "read",
                serde_json::json!({"path": format!("src/file_{index}.rs")}),
            ));
            seeded_messages.push(make_tool_result(&call_id, "read", &"x".repeat(400)));
        }

        let mut usage_messages = seeded_messages.clone();
        usage_messages.push(Message::user("trigger masking"));
        let provisional_model = test_model(provider.clone());
        let usage_before = crate::context::context_usage(&usage_messages, &provisional_model);

        let mut masked_messages = usage_messages.clone();
        crate::context::mask_observations(&mut masked_messages, 10);
        let usage_after = crate::context::context_usage(&masked_messages, &provisional_model);

        assert!(usage_before.used > usage_after.used);

        // Pick a window where masking definitely triggers.
        let context_window = ((usage_before.used as f64) / 0.7).ceil() as u32;

        let model = test_model_with_context_window(provider, context_window.max(1));
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        let events_task = tokio::spawn(collect_events(handle));
        agent.messages = seeded_messages;

        agent.run("trigger masking".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnStart { index: 0 })),
            "agent should still run normally"
        );
    }

    #[tokio::test]
    async fn agent_reports_context_full_before_provider_request() {
        let provider = Arc::new(MockProvider::new(vec![text_response(
            "should not be called",
            1,
            1,
        )]));
        let model = test_model_with_context_window(provider, 1);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        let events_task = tokio::spawn(collect_events(handle));

        let result = agent
            .run("this message is definitely too large".to_string())
            .await;
        drop(agent);

        assert!(matches!(
            result,
            Err(crate::error::Error::Llm(
                imp_llm::Error::ContextTooLong { .. }
            ))
        ));

        let events = events_task.await.unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::Error { error }
                if error.contains("Context full") && error.contains("Run /compact")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::AgentEnd {
                status: RunFinalStatus::Failed { message },
                ..
            } if message.contains("Context full")
        )));
    }

    // ── Usage/cost accumulation ────────────────────────────────────

    #[tokio::test]
    async fn agent_usage_cost_accumulation() {
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "echo",
                serde_json::json!({"text": "a"}),
                1_000_000, // 1M input tokens
                500_000,   // 500k output tokens
            ),
            text_response("done", 1_000_000, 500_000),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.tools.register(Arc::new(EchoTool));

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("test".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        if let Some(AgentEvent::AgentEnd { usage, cost, .. }) = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        {
            // 2M input, 1M output
            assert_eq!(usage.input_tokens, 2_000_000);
            assert_eq!(usage.output_tokens, 1_000_000);

            // Cost: 2M * $3/Mtok input = $6, 1M * $15/Mtok output = $15, total = $21
            assert!((cost.input - 6.0).abs() < 1e-10);
            assert!((cost.output - 15.0).abs() < 1e-10);
            assert!((cost.total - 21.0).abs() < 1e-10);
        } else {
            panic!("Expected AgentEnd");
        }
    }

    // ── Retry policy tests ─────────────────────────────────────────

    /// A mock provider that returns a fixed sequence of results. Each call to
    /// `stream()` returns the next item: an `Err` for errors, or a pre-built
    /// event sequence for success.
    struct RetryMockProvider {
        calls: Mutex<Vec<std::result::Result<Vec<StreamEvent>, imp_llm::Error>>>,
    }

    impl RetryMockProvider {
        fn new(calls: Vec<std::result::Result<Vec<StreamEvent>, imp_llm::Error>>) -> Self {
            Self {
                calls: Mutex::new(calls),
            }
        }
    }

    #[async_trait]
    impl Provider for RetryMockProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            let mut calls = self.calls.try_lock().expect("RetryMockProvider lock");
            let outcome = if calls.is_empty() {
                Ok(vec![StreamEvent::Error {
                    error: "No more mock responses".to_string(),
                }])
            } else {
                calls.remove(0)
            };
            match outcome {
                Ok(events) => Box::pin(futures::stream::iter(
                    events.into_iter().map(imp_llm::Result::Ok),
                )),
                Err(e) => Box::pin(futures::stream::once(async move {
                    imp_llm::Result::<StreamEvent>::Err(e)
                })),
            }
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("mock-key".to_string())
        }

        fn id(&self) -> &str {
            "retry-mock"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    /// Provider that fails N times with a rate-limit error, then succeeds.
    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        use imp_llm::provider::RetryPolicy;

        let provider = Arc::new(RetryMockProvider::new(vec![
            // First two calls fail with a rate-limit error
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
            // Third call succeeds
            Ok(text_response("Hello after retries", 100, 20)),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        // Zero delays so the test runs fast
        agent.retry_policy = RetryPolicy {
            max_retries: 3,
            base_delay: std::time::Duration::from_millis(0),
            max_delay: std::time::Duration::from_secs(30),
            retry_on: vec![],
        };

        let events_task = tokio::spawn(collect_events(handle));
        agent.run("Say hello".to_string()).await.unwrap();
        drop(agent);

        let events = events_task.await.unwrap();

        // Agent should have completed successfully
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));

        // The final text should be present in TurnEnd
        let turn_end = events.iter().find_map(|e| match e {
            AgentEvent::TurnEnd { message, .. } => Some(message),
            _ => None,
        });
        assert!(turn_end.is_some());
        let content_text = turn_end
            .unwrap()
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert!(
            content_text.contains("Hello after retries"),
            "expected final text, got: {content_text}"
        );
    }

    /// When max_retries is exhausted the agent returns an error.
    #[tokio::test]
    async fn retry_fails_when_max_retries_exhausted() {
        use imp_llm::provider::RetryPolicy;

        let provider = Arc::new(RetryMockProvider::new(vec![
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
            Err(imp_llm::Error::RateLimited {
                retry_after_secs: Some(0),
            }),
        ]));

        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.retry_policy = RetryPolicy {
            max_retries: 2, // only 2 retries allowed
            base_delay: std::time::Duration::from_millis(0),
            max_delay: std::time::Duration::from_secs(30),
            retry_on: vec![],
        };
        drop(handle);

        let result = agent.run("Fail".to_string()).await;
        assert!(
            result.is_err(),
            "should have failed after exhausting retries"
        );
    }

    /// Auth errors (HTTP 401/403) must NOT be retried.
    #[tokio::test]
    async fn retry_does_not_retry_auth_errors() {
        use imp_llm::provider::RetryPolicy;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        struct CountingAuthFailProvider {
            calls: AtomicUsize,
            success_after: usize,
        }

        #[async_trait]
        impl Provider for CountingAuthFailProvider {
            fn stream(
                &self,
                _model: &Model,
                _context: Context,
                _options: RequestOptions,
                _api_key: &str,
            ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n < self.success_after {
                    Box::pin(futures::stream::once(async {
                        Err(imp_llm::Error::Auth("Invalid API key".to_string()))
                    }))
                } else {
                    Box::pin(futures::stream::iter(
                        text_response("ok", 10, 5).into_iter().map(Ok),
                    ))
                }
            }

            async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
                Ok("mock-key".to_string())
            }

            fn id(&self) -> &str {
                "auth-fail-mock"
            }

            fn models(&self) -> &[ModelMeta] {
                &[]
            }
        }

        let _ = call_count_clone; // silence unused warning

        let provider = Arc::new(CountingAuthFailProvider {
            calls: AtomicUsize::new(0),
            success_after: 999, // would succeed eventually, but we expect no retry
        });
        let call_ref = &provider.calls;

        let model = test_model(provider.clone());
        let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
        agent.retry_policy = RetryPolicy {
            max_retries: 5, // generous, to confirm auth errors bypass retry entirely
            base_delay: std::time::Duration::from_millis(0),
            max_delay: std::time::Duration::from_secs(30),
            retry_on: vec![],
        };
        drop(handle);

        let result = agent.run("Auth test".to_string()).await;
        assert!(result.is_err(), "should fail on auth error");

        // The provider should have been called exactly once — no retries.
        assert_eq!(
            call_ref.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "auth errors should not be retried"
        );
    }
}

// ── Integration tests: full ReAct cycle with real tools ─────────────

#[cfg(test)]
mod integration {
    use super::*;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::auth::{ApiKey, AuthStore};
    use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
    use imp_llm::provider::Provider;
    use tokio::sync::Mutex;

    use crate::tools::{bash::BashTool, edit::EditTool, read::ReadTool, write::WriteTool};

    // ── Shared test helpers (duplicated from unit tests to keep modules independent) ──

    struct MockProvider {
        responses: Mutex<Vec<Vec<StreamEvent>>>,
    }

    impl MockProvider {
        fn new(responses: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            let mut responses = self.responses.try_lock().expect("MockProvider lock");
            let events = if responses.is_empty() {
                vec![StreamEvent::Error {
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

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    fn test_model(provider: Arc<dyn Provider>) -> Model {
        Model {
            meta: ModelMeta {
                id: "test-model".to_string(),
                provider: "mock".to_string(),
                name: "Test Model".to_string(),
                context_window: 200_000,
                max_output_tokens: 16_384,
                pricing: ModelPricing {
                    input_per_mtok: 3.0,
                    output_per_mtok: 15.0,
                    cache_read_per_mtok: 0.3,
                    cache_write_per_mtok: 3.75,
                },
                capabilities: Capabilities {
                    reasoning: true,
                    images: false,
                    tool_use: true,
                },
            },
            provider,
        }
    }

    fn text_response(text: &str, input_tokens: u32, output_tokens: u32) -> Vec<StreamEvent> {
        vec![
            StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            StreamEvent::TextDelta {
                text: text.to_string(),
            },
            StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    usage: Some(Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            },
        ]
    }

    fn tool_call_response(
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Vec<StreamEvent> {
        vec![
            StreamEvent::MessageStart {
                model: "test-model".to_string(),
            },
            StreamEvent::ToolCall {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments: args.clone(),
            },
            StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::ToolCall {
                        id: call_id.to_string(),
                        name: tool_name.to_string(),
                        arguments: args,
                    }],
                    usage: Some(Usage {
                        input_tokens,
                        output_tokens,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::ToolUse,
                    timestamp: 1000,
                },
            },
        ]
    }

    /// Create an agent pre-loaded with the reduced default tool set used by tests.
    fn create_agent_with_tools(provider: Arc<dyn Provider>, cwd: PathBuf) -> (Agent, AgentHandle) {
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, cwd);
        agent.tools.register(Arc::new(WriteTool));
        agent.tools.register(Arc::new(ReadTool));
        agent.tools.register(Arc::new(EditTool));
        agent.tools.register(Arc::new(BashTool));
        (agent, handle)
    }

    /// Create an agent with reduced tools only (used for synthetic A/B tests).
    fn create_agent_with_reduced_tools(
        provider: Arc<dyn Provider>,
        cwd: PathBuf,
    ) -> (Agent, AgentHandle) {
        let model = test_model(provider);
        let (mut agent, handle) = Agent::new(model, cwd);
        agent.tools.register(Arc::new(WriteTool));
        agent.tools.register(Arc::new(ReadTool));
        agent.tools.register(Arc::new(EditTool));
        agent.tools.register(Arc::new(BashTool));
        (agent, handle)
    }

    // ── Test 1: Write then read a file ─────────────────────────────

    #[tokio::test]
    async fn agent_reads_and_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_write",
                "write",
                serde_json::json!({"path": "test.txt", "content": "hello world"}),
                100,
                20,
            ),
            tool_call_response(
                "call_read",
                "read",
                serde_json::json!({"path": "test.txt"}),
                100,
                20,
            ),
            text_response("The file contains: hello world", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
        ]));

        let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent
            .run("Write and read a file".to_string())
            .await
            .unwrap();

        // File should exist on disk with correct content
        let on_disk = std::fs::read_to_string(tmp.path().join("test.txt")).unwrap();
        assert_eq!(on_disk, "hello world");

        // Read tool result should contain the file content
        let read_result = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_read" => Some(r),
                _ => None,
            })
            .expect("should have a read tool result");
        let read_text = read_result
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert!(
            read_text.contains("hello world"),
            "read result should contain file content, got: {read_text}"
        );

        // Assistant messages should include the write, read, and final text turns.
        let assistant_count = agent
            .messages
            .iter()
            .filter(|m| matches!(m, Message::Assistant(_)))
            .count();
        assert!(
            assistant_count >= 3,
            "got {assistant_count} assistant messages"
        );
    }

    // ── Test 2: Edit tool modifies a file ──────────────────────────

    #[tokio::test]
    async fn agent_edit_tool_modifies_file() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_write",
                "write",
                serde_json::json!({
                    "path": "src/main.rs",
                    "content": "fn main() {\n    println!(\"old\");\n}"
                }),
                100,
                20,
            ),
            tool_call_response(
                "call_edit",
                "edit",
                serde_json::json!({
                    "path": "src/main.rs",
                    "oldText": "old",
                    "newText": "new"
                }),
                100,
                20,
            ),
            tool_call_response(
                "call_read",
                "read",
                serde_json::json!({"path": "src/main.rs"}),
                100,
                20,
            ),
            text_response("Done", 100, 20),
            text_response("Done", 100, 20),
            text_response("Done", 100, 20),
        ]));

        let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent.run("Edit a file".to_string()).await.unwrap();

        // File should contain "new" not "old"
        let on_disk = std::fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
        assert!(on_disk.contains("new"), "file should contain 'new'");
        assert!(!on_disk.contains("old"), "file should not contain 'old'");

        // Edit tool result should include a diff
        let edit_result = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_edit" => Some(r),
                _ => None,
            })
            .expect("should have an edit tool result");
        let edit_text = edit_result
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert!(
            edit_text.contains("---") || edit_text.contains("+++"),
            "edit result should include a diff, got: {edit_text}"
        );
    }

    // ── Test 3: Bash search finds a pattern (synthetic A/B baseline) ──────

    #[tokio::test]
    async fn agent_bash_search_finds_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("search_me.txt"),
            "line one\nunique_pattern_xyz here\nline three\n",
        )
        .unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_bash",
                "bash",
                serde_json::json!({"command": "grep --no-color -rn 'unique_pattern_xyz' ."}),
                100,
                20,
            ),
            text_response("Found it!", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
            text_response("Done.", 100, 20),
        ]));

        let (mut agent, handle) =
            create_agent_with_reduced_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent.run("Search for a pattern".to_string()).await.unwrap();

        let bash_result = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_bash" => Some(r),
                _ => None,
            })
            .expect("should have a bash tool result");
        let bash_text = bash_result
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert!(
            !bash_text.trim().is_empty(),
            "bash grep output should not be empty"
        );
    }

    // ── Test 3b: repeated identical tool calls warn and then block ────────

    #[tokio::test]
    async fn agent_repeated_tool_calls_warn_then_block() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("repeat.txt"), "same content\n").unwrap();

        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_1",
                "read",
                serde_json::json!({"path": "repeat.txt"}),
                100,
                20,
            ),
            tool_call_response(
                "call_2",
                "read",
                serde_json::json!({"path": "repeat.txt"}),
                100,
                20,
            ),
            tool_call_response(
                "call_3",
                "read",
                serde_json::json!({"path": "repeat.txt"}),
                100,
                20,
            ),
            tool_call_response(
                "call_4",
                "read",
                serde_json::json!({"path": "repeat.txt"}),
                100,
                20,
            ),
            text_response("Done", 100, 20),
        ]));

        let (mut agent, handle) =
            create_agent_with_reduced_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent
            .run("Read the same file repeatedly".to_string())
            .await
            .unwrap();

        let third = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_3" => Some(r),
                _ => None,
            })
            .expect("third tool result");
        let fourth = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_4" => Some(r),
                _ => None,
            })
            .expect("fourth tool result");

        let third_text = third
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let fourth_text = fourth
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(third_text.contains("Warning: identical tool call repeated 3 times"));
        assert!(fourth.is_error);
        assert!(fourth_text.contains("Blocked: identical tool call repeated 4 times"));
        assert_eq!(
            agent
                .messages
                .iter()
                .filter(|message| matches!(message, Message::User(_)))
                .count(),
            1,
            "agent should stop after repeated-action block rather than enqueueing more follow-ups"
        );
    }

    #[test]
    fn tool_results_indicate_repeated_action_detects_blocked_repeat_message() {
        let result = imp_llm::ToolResultMessage {
            tool_call_id: "call_repeat".to_string(),
            tool_name: "read".to_string(),
            content: vec![ContentBlock::Text {
                text: "Blocked: identical tool call repeated 4 times in a row for 'read'."
                    .to_string(),
            }],
            is_error: true,
            details: serde_json::Value::Null,
            timestamp: 0,
        };

        assert!(tool_results_indicate_repeated_action(&[result]));
    }

    // ── Test 4: Bash runs a command ────────────────────────────────

    #[tokio::test]
    async fn agent_bash_runs_command() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_bash",
                "bash",
                serde_json::json!({"command": "echo hello && echo world"}),
                100,
                20,
            ),
            text_response("Done", 100, 20),
        ]));

        let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent.run("Run a command".to_string()).await.unwrap();

        // Bash result should contain the command output
        let bash_result = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_bash" => Some(r),
                _ => None,
            })
            .expect("should have a bash tool result");
        let bash_text = bash_result
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert!(
            bash_text.contains("hello"),
            "bash output should contain 'hello', got: {bash_text}"
        );
        assert!(
            bash_text.contains("world"),
            "bash output should contain 'world', got: {bash_text}"
        );

        // Details should include exit_code: 0
        assert_eq!(bash_result.details["exit_code"], 0);
    }

    // ── Test 5: Tool error → agent self-corrects ───────────────────

    #[tokio::test]
    async fn agent_handles_tool_error_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = Arc::new(MockProvider::new(vec![
            tool_call_response(
                "call_read",
                "read",
                serde_json::json!({"path": "nonexistent.txt"}),
                100,
                20,
            ),
            text_response("File not found, let me try something else", 100, 20),
        ]));

        let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
        drop(handle);

        agent.run("Read a file".to_string()).await.unwrap();

        // Read tool result should have is_error=true
        let read_result = agent
            .messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult(r) if r.tool_call_id == "call_read" => Some(r),
                _ => None,
            })
            .expect("should have a read tool result");
        assert!(
            read_result.is_error,
            "reading nonexistent file should produce an error result"
        );

        // Agent should continue to turn 1 and self-correct with text
        let assistant_count = agent
            .messages
            .iter()
            .filter(|m| matches!(m, Message::Assistant(_)))
            .count();
        assert_eq!(
            assistant_count, 2,
            "agent should have 2 turns: error + recovery"
        );

        // Agent completed successfully (no Err return)
    }
}

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
            personality: None,
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
            personality: None,
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
            personality: None,
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
            personality: None,
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
            personality: None,
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
