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
mod context_recovery;
mod current_task_state;
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
    assistant_message_contains_workflow_tool_call, assistant_message_text,
    bash_result_is_successful_check, should_queue_confidence_continue_follow_up,
    should_queue_execution_debt_follow_up, tool_results_include_successful_check,
    tool_results_include_successful_edit, tool_results_indicate_execution_blocker,
    tool_results_indicate_failed_bash_command, tool_results_indicate_repeated_action,
    tool_results_indicate_work_completed, ContinueRecommendation, PostTurnAssessment,
    RuntimeEvidence, TextFallbackEvidence, WorkflowEvidence,
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
    /// Provider-observed effective input ceiling for this session. Set when a
    /// provider rejects a request below our configured model limit so later
    /// turns trim/compact before hitting the same backend limit again.
    pub observed_context_input_limit: Option<u32>,
    /// Provider-reported context baseline from the last successful response.
    /// OpenAI/Codex can account for hidden or encrypted reasoning that is not
    /// represented in imp's logical message history, so this baseline is used
    /// as an authoritative floor for later local estimates until the local
    /// history is rewritten by masking or compaction.
    pub provider_context_baseline_tokens: Option<u32>,
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
            observed_context_input_limit: None,
            provider_context_baseline_tokens: None,
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

fn confidence_continue_follow_up_text() -> &'static str {
    "Confidence is high and the workflow delta is already visible. Continue to the next small, well-bounded step now using the native workflow-backed process, unless a consequential decision or blocker appears. Do not re-summarize the same visible workflow change in chat unless new context needs to be called out."
}

fn failed_bash_recovery_follow_up_text() -> &'static str {
    "The last bash command failed, but a failed command is usually diagnostic evidence, not a stopping condition. Inspect the command output, identify the root cause, make the smallest useful fix or choose a better command, and rerun the relevant check. Stop only if the failure proves a concrete blocker that needs user input."
}

fn execution_debt_follow_up_text() -> &'static str {
    "You have recorded or planned work, but the requested outcome is not satisfied yet. Continue working until the user's requested outcome is satisfied, or until concrete evidence shows it cannot be completed. Do not stop merely because you recorded a plan, updated a workflow, or completed one intermediate step."
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
#[path = "mode_tests.rs"]
mod mode_tests;
