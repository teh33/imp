use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use imp_llm::{
    AssistantMessage, ContentBlock, Context, Message, RequestOptions, StopReason, StreamEvent,
    Usage,
};

use crate::agent::loop_state::enforce_verification_closeout;
use crate::agent::{
    Agent, AgentCommand, AgentEvent, LoopDecision, RecoveryCheckpointKind, RunFinalStatus,
    StopReason as AgentStopReason, TimingEvent, TimingStage, TurnPhase, TurnState,
};
use crate::config::AutoCompactionMode;
use crate::error::Result;
use crate::hooks::HookEvent;
use crate::ui::NotifyLevel;
use crate::workflow::{
    capture_worktree_diff_artifacts, write_worktree_metadata, VerificationGateRunner,
    WorktreeRunMetadata,
};
use crate::{storage, trace::TraceWriter};

use super::{
    build_assistant_message, clone_model, push_stream_text_block, push_stream_thinking_block,
};

const STREAM_RECOVERY_FOLLOW_UP: &str = "The provider stream failed before completing the previous assistant message. Continue from the last completed conversation state. Do not repeat already completed tool side effects; if you need to retry, first inspect current state and proceed safely.";
const MAX_STREAM_RECOVERY_ATTEMPTS: u32 = 2;
const MAX_CONTEXT_RECOVERY_ATTEMPTS: u32 = 1;

fn recoverable_stream_failure_message(error: &str) -> Option<String> {
    if error.contains("Provider stream failed after partial output")
        || error.contains("Provider stream failed before output")
        || error.contains("missing terminal completion event")
    {
        Some(format!(
            "{STREAM_RECOVERY_FOLLOW_UP}\n\nProvider error: {error}"
        ))
    } else {
        None
    }
}

fn effective_display_window(
    estimate: &crate::context::RequestContextEstimate,
    observed_input_limit: Option<u32>,
) -> u32 {
    observed_input_limit
        .map(|limit| estimate.display_window.min(limit.max(1)))
        .unwrap_or(estimate.display_window)
}

fn format_context_estimate_details(
    estimate: &crate::context::RequestContextEstimate,
    observed_input_limit: Option<u32>,
) -> String {
    let effective = observed_input_limit.unwrap_or(estimate.input_limit);
    let display = effective_display_window(estimate, observed_input_limit);
    format!(
        "estimate: input {} / effective limit {} (model limit {}, display {}, system {}, tools {}, messages {}, planned output {}, observed ceiling {})",
        estimate.input_tokens,
        effective,
        estimate.input_limit,
        display,
        estimate.system_tokens,
        estimate.tool_definition_tokens,
        estimate.message_tokens,
        estimate.output_tokens,
        observed_input_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "none".to_string())
    )
}

fn recoverable_context_failure_message(error: &str) -> bool {
    crate::error_display::format_error_for_display(error).starts_with("Context full:")
}

fn recoverable_context_failure(error: &imp_llm::Error) -> bool {
    matches!(error, imp_llm::Error::ContextTooLong { .. })
        || matches!(error, imp_llm::Error::Provider(message) if recoverable_context_failure_message(message))
}

fn auto_compaction_should_run(
    mode: AutoCompactionMode,
    usage: &crate::context::ContextUsage,
    trigger_ratio: f64,
) -> bool {
    if usage.limit == 0 {
        return false;
    }
    let trigger_ratio = if trigger_ratio.is_finite() {
        trigger_ratio.clamp(0.0, 1.0)
    } else {
        0.90
    };
    match mode {
        AutoCompactionMode::Disabled => false,
        AutoCompactionMode::NearThreshold => usage.ratio >= trigger_ratio,
        AutoCompactionMode::Aggressive => usage.ratio >= trigger_ratio.min(0.75),
    }
}

fn auto_compaction_tail_tokens(usage: &crate::context::ContextUsage, target_ratio: f64) -> u32 {
    if usage.limit == 0 {
        return crate::compaction::AUTO_COMPACTION_RECENT_TAIL_TOKENS;
    }
    let target_ratio = if target_ratio.is_finite() {
        target_ratio.clamp(0.05, 0.95)
    } else {
        0.70
    };
    let target = (usage.limit as f64 * target_ratio).floor() as u32;
    target
        .max(16_000)
        .min(crate::compaction::AUTO_COMPACTION_RECENT_TAIL_TOKENS)
}

fn observed_input_limit_after_overflow(estimate: &crate::context::RequestContextEstimate) -> u32 {
    // If the provider rejects a request below our configured model limit, treat
    // that provider response as authoritative for this run and leave headroom
    // below the failed local estimate. This prevents imp from repeatedly
    // waiting until the same too-high local token count before trimming again.
    ((estimate.input_tokens as f64) * 0.80).floor().max(1.0) as u32
}

fn update_observed_input_limit(
    observed_input_limit: &mut Option<u32>,
    estimate: &crate::context::RequestContextEstimate,
) -> u32 {
    let observed = observed_input_limit_after_overflow(estimate);
    let effective = observed_input_limit
        .map(|existing| existing.min(observed))
        .unwrap_or(observed);
    *observed_input_limit = Some(effective);
    effective
}

fn apply_provider_context_baseline(
    estimate: &mut crate::context::RequestContextEstimate,
    provider_context_baseline_tokens: Option<u32>,
) {
    if let Some(baseline) = provider_context_baseline_tokens {
        estimate.input_tokens = estimate.input_tokens.max(baseline);
    }
}

fn sanitized_request_estimate(
    messages: &[Message],
    model: &imp_llm::Model,
    options: &RequestOptions,
    observed_input_limit: Option<u32>,
    provider_context_baseline_tokens: Option<u32>,
) -> (Vec<Message>, crate::context::RequestContextEstimate) {
    let mut context_messages = messages.to_vec();
    crate::session::sanitize_messages(&mut context_messages);
    let mut estimate = crate::context::estimate_request_context(&context_messages, model, options);
    apply_provider_context_baseline(&mut estimate, provider_context_baseline_tokens);
    if let Some(limit) = observed_input_limit {
        let effective_limit = limit.max(1);
        estimate.input_limit = estimate.input_limit.min(effective_limit);
        estimate.display_window = estimate.display_window.min(effective_limit);
    }
    (context_messages, estimate)
}

fn mask_all_observations_for_recovery(
    messages: &mut [Message],
    model: &imp_llm::Model,
    options: &RequestOptions,
    observed_input_limit: Option<u32>,
    provider_context_baseline_tokens: Option<u32>,
) -> Option<(u32, u32)> {
    let before = sanitized_request_estimate(
        messages,
        model,
        options,
        observed_input_limit,
        provider_context_baseline_tokens,
    )
    .1
    .input_tokens;
    crate::context::mask_observations(messages, 0);
    let after = sanitized_request_estimate(messages, model, options, observed_input_limit, None)
        .1
        .input_tokens;
    (after < before).then_some((before, after))
}

impl Agent {
    pub(super) async fn reconcile_recovery_before_turn(
        &self,
        turn: u32,
    ) -> Option<super::RecoveryReconciliation> {
        let reconciliation = self
            .recovery_ledger
            .lock()
            .ok()
            .and_then(|ledger| ledger.reconcile_latest_finished_turn())?;

        // Only a previous turn can block the next turn. The current turn has no
        // side effects yet, and same-turn reconciliation happens after tool
        // execution checkpoints are recorded.
        if reconciliation.turn >= turn {
            return None;
        }

        if !reconciliation.is_safe_to_continue() {
            self.emit(AgentEvent::Error {
                error: format!(
                    "Recovery blocked before turn {turn}: {} incomplete non-retryable tool side effect(s)",
                    reconciliation.unsafe_incomplete_tools.len()
                ),
            })
            .await;
        }

        Some(reconciliation)
    }

    async fn run_verification_gates(&mut self, artifacts: &storage::RunArtifacts) {
        let runner = VerificationGateRunner::new(&self.cwd, artifacts.root().join("verification"));
        let mut completed = Vec::new();
        for index in 0..self.verification_gates.len() {
            if matches!(
                self.verification_gates[index].status,
                crate::workflow::VerificationGateStatus::Passed
                    | crate::workflow::VerificationGateStatus::Failed
                    | crate::workflow::VerificationGateStatus::Blocked
                    | crate::workflow::VerificationGateStatus::Skipped
            ) {
                continue;
            }
            self.emit(AgentEvent::VerificationStarted {
                gate: self.verification_gates[index].clone(),
            })
            .await;
            let _ = runner.run(&mut self.verification_gates[index]).await;
            completed.push(self.verification_gates[index].clone());
        }
        for gate in completed {
            self.emit(AgentEvent::VerificationCompleted {
                closeout_effect: gate.closeout_effect(),
                gate,
            })
            .await;
        }
    }

    async fn capture_worktree_run_artifacts(
        &self,
        artifacts: &storage::RunArtifacts,
    ) -> Option<WorktreeRunMetadata> {
        let mut metadata = self.worktree_run_metadata.clone()?;
        self.write_trace_event(&AgentEvent::WorktreeCreated {
            metadata: metadata.clone(),
        });
        let worktree_artifact_dir = artifacts.root().join("worktree");
        match capture_worktree_diff_artifacts(&mut metadata, &worktree_artifact_dir).await {
            Ok(_) => {
                let _ = write_worktree_metadata(
                    &worktree_artifact_dir.join("worktree-metadata.json"),
                    &metadata,
                )
                .await;
                self.write_trace_event(&AgentEvent::WorktreeDiffCaptured {
                    metadata: metadata.clone(),
                });
                Some(metadata)
            }
            Err(err) => {
                self.write_trace_event(&AgentEvent::Warning {
                    message: format!("failed to capture worktree diff artifacts: {err}"),
                });
                None
            }
        }
    }

    pub async fn run(&mut self, prompt: String) -> Result<()> {
        let trace_path = std::env::var_os("IMP_TUI_TRACE").map(std::path::PathBuf::from);
        let trace_run = |phase: &str, started: std::time::Instant| {
            if let Some(path) = trace_path.as_ref() {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write as _;
                    let _ = writeln!(
                        file,
                        "{} agent_run_phase phase={} duration_ms={}",
                        imp_llm::now(),
                        phase,
                        started.elapsed().as_millis()
                    );
                }
            }
        };
        let phase_started = std::time::Instant::now();
        let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
        let run_artifacts = storage::project_run_artifacts(&self.cwd, &run_id).ok();
        if let Some(artifacts) = &run_artifacts {
            if let Ok(writer) = TraceWriter::create(artifacts.trace_path()) {
                if let Ok(mut active_trace_writer) = self.trace_writer.lock() {
                    *active_trace_writer = Some(writer);
                }
            }
            self.write_workflow_contract_snapshot(artifacts);
        }
        trace_run("artifacts", phase_started);
        let phase_started = std::time::Instant::now();
        if let Ok(mut active_run_id) = self.run_id.lock() {
            *active_run_id = Some(run_id.clone());
        }
        trace_run("set_run_id", phase_started);
        let phase_started = std::time::Instant::now();

        self.emit(AgentEvent::AgentStart {
            model: self.model.meta.id.clone(),
            timestamp: imp_llm::now(),
        })
        .await;
        trace_run("emit_agent_start", phase_started);
        let phase_started = std::time::Instant::now();
        self.hooks
            .fire(&HookEvent::OnAgentStart { prompt: &prompt })
            .await;
        trace_run("hook_agent_start", phase_started);
        let phase_started = std::time::Instant::now();

        self.active_objective = Some(super::AutonomousObjective::from_prompt(&prompt));
        self.messages.push(Message::user(&prompt));

        self.cancel_token
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let mut turn: u32 = 0;
        let mut total_usage = Usage::default();
        let mut cancelled = false;
        let mut final_status: Option<RunFinalStatus> = None;
        let mut queued_follow_ups: std::collections::VecDeque<String> =
            std::collections::VecDeque::new();
        let mut queued_pre_turn_follow_ups: std::collections::VecDeque<String> =
            std::collections::VecDeque::new();
        let mut stream_recovery_attempts: u32 = 0;
        let mut context_recovery_attempts: u32 = 0;
        let mut observed_input_limit: Option<u32> = self.observed_context_input_limit;
        trace_run("init_loop_state", phase_started);

        if let Some(nudge) = self.workflow_pre_turn_follow_up_hint(&prompt, !self.tools.is_empty())
        {
            queued_pre_turn_follow_ups.push_back(nudge.to_string());
        }

        'turns: loop {
            self.begin_workflow_turn(&prompt, run_artifacts.as_ref())
                .await;
            let mut turn_state = TurnState::new(turn);
            turn_state.enter(TurnPhase::ReceiveCommands);

            if let Some(reconciliation) = self.reconcile_recovery_before_turn(turn).await {
                if !reconciliation.is_safe_to_continue() {
                    let unsafe_count = reconciliation.unsafe_incomplete_tools.len();
                    final_status = Some(RunFinalStatus::Blocked {
                        reason: AgentStopReason::ExecutionBlocked,
                        message: format!(
                            "recovery requires user review: {unsafe_count} incomplete non-retryable tool side effect(s)"
                        ),
                    });
                    break;
                }
            }

            if turn > 0 {
                if let Some(follow_up) = queued_pre_turn_follow_ups
                    .pop_front()
                    .or_else(|| queued_follow_ups.pop_front())
                {
                    turn_state.record_continue(super::ContinueReason::QueuedUserFollowUp);
                    self.messages.push(Message::user(&follow_up));
                }
            }

            // Check for commands between turns (non-blocking)
            while let Ok(cmd) = self.command_rx.try_recv() {
                match cmd {
                    AgentCommand::Cancel => {
                        self.cancel_token
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        cancelled = true;
                        break;
                    }
                    AgentCommand::Steer(msg) => {
                        self.messages.push(Message::user(&msg));
                    }
                    AgentCommand::FollowUp(msg) => queued_follow_ups.push_back(msg),
                }
            }

            if cancelled {
                break;
            }

            turn_state.enter(TurnPhase::PreTurn);
            self.emit(AgentEvent::TurnStart { index: turn }).await;
            self.begin_turn_workflow_review(turn);
            let turn_started_at = Instant::now();
            turn_state.enter(TurnPhase::BuildContext);
            self.emit_timing(
                turn,
                TimingStage::ContextAssemblyStart,
                turn_started_at,
                None,
            )
            .await;
            let context_assembly_started_at = Instant::now();

            let options = RequestOptions {
                thinking_level: self.thinking_level,
                // Use configured output cap when present; otherwise let providers
                // choose their own sensible default output budget.
                max_tokens: self.max_tokens,
                temperature: None,
                system_prompt: self.system_prompt.clone(),
                tools: self.tools.definitions(),
                cache_options: self.cache_options.clone(),
                effort: None,
            };

            let (mut context_messages, mut request_estimate) = sanitized_request_estimate(
                &self.messages,
                &self.model,
                &options,
                observed_input_limit,
                self.provider_context_baseline_tokens,
            );
            self.emit(AgentEvent::ContextUsageUpdated {
                used: request_estimate.input_tokens,
                display_window: request_estimate.display_window,
                input_limit: request_estimate.input_limit,
                system_tokens: request_estimate.system_tokens,
                tool_definition_tokens: request_estimate.tool_definition_tokens,
                message_tokens: request_estimate.message_tokens,
                output_tokens: request_estimate.output_tokens,
                observed_input_limit,
            })
            .await;
            let mut usage = request_estimate.as_usage();
            if usage.ratio >= self.context_config.observation_mask_threshold {
                crate::context::mask_observations(
                    &mut self.messages,
                    self.context_config.mask_window,
                );
                self.provider_context_baseline_tokens = None;
                self.hooks
                    .fire(&HookEvent::OnContextThreshold { ratio: usage.ratio })
                    .await;
                // Masking can materially reduce context size, so any subsequent
                // logic must use fresh exact-request usage rather than the
                // pre-masking snapshot.
                (context_messages, request_estimate) = sanitized_request_estimate(
                    &self.messages,
                    &self.model,
                    &options,
                    observed_input_limit,
                    self.provider_context_baseline_tokens,
                );
                self.emit(AgentEvent::ContextUsageUpdated {
                    used: request_estimate.input_tokens,
                    display_window: request_estimate.display_window,
                    input_limit: request_estimate.input_limit,
                    system_tokens: request_estimate.system_tokens,
                    tool_definition_tokens: request_estimate.tool_definition_tokens,
                    message_tokens: request_estimate.message_tokens,
                    output_tokens: request_estimate.output_tokens,
                    observed_input_limit,
                })
                .await;
                usage = request_estimate.as_usage();
            }

            if auto_compaction_should_run(
                self.context_config.auto_compaction.mode,
                &usage,
                self.context_config.auto_compaction.trigger_ratio,
            ) {
                let recent_tail_tokens = auto_compaction_tail_tokens(
                    &usage,
                    self.context_config.auto_compaction.target_ratio,
                );
                if let Some(compaction) = crate::compaction::compact_messages_for_auto_compaction(
                    &self.messages,
                    &self.model,
                    recent_tail_tokens,
                ) {
                    self.messages = compaction.messages;
                    self.provider_context_baseline_tokens = None;
                    self.emit(AgentEvent::Warning {
                        message: format!(
                            "Auto-compacted older tool output and file dumps before the provider request ({} -> {} estimated tokens). Exact old output may need to be reread.",
                            compaction.tokens_before,
                            compaction.tokens_after
                        ),
                    })
                    .await;
                    (context_messages, request_estimate) = sanitized_request_estimate(
                        &self.messages,
                        &self.model,
                        &options,
                        observed_input_limit,
                        self.provider_context_baseline_tokens,
                    );
                    self.emit(AgentEvent::ContextUsageUpdated {
                        used: request_estimate.input_tokens,
                        display_window: request_estimate.display_window,
                        input_limit: request_estimate.input_limit,
                        system_tokens: request_estimate.system_tokens,
                        tool_definition_tokens: request_estimate.tool_definition_tokens,
                        message_tokens: request_estimate.message_tokens,
                        output_tokens: request_estimate.output_tokens,
                        observed_input_limit,
                    })
                    .await;
                    usage = request_estimate.as_usage();
                }
            }

            if usage.used >= usage.limit && usage.limit > 0 {
                if context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS {
                    if let Some((before, after)) = mask_all_observations_for_recovery(
                        &mut self.messages,
                        &self.model,
                        &options,
                        observed_input_limit,
                        self.provider_context_baseline_tokens,
                    ) {
                        context_recovery_attempts += 1;
                        self.provider_context_baseline_tokens = None;
                        self.emit(AgentEvent::Warning {
                            message: format!(
                                "Context recovery masked tool outputs before the provider request ({before} -> {after} estimated tokens)."
                            ),
                        })
                        .await;
                        (context_messages, request_estimate) = sanitized_request_estimate(
                            &self.messages,
                            &self.model,
                            &options,
                            observed_input_limit,
                            self.provider_context_baseline_tokens,
                        );
                        self.emit(AgentEvent::ContextUsageUpdated {
                            used: request_estimate.input_tokens,
                            display_window: request_estimate.display_window,
                            input_limit: request_estimate.input_limit,
                            system_tokens: request_estimate.system_tokens,
                            tool_definition_tokens: request_estimate.tool_definition_tokens,
                            message_tokens: request_estimate.message_tokens,
                            output_tokens: request_estimate.output_tokens,
                            observed_input_limit,
                        })
                        .await;
                        usage = request_estimate.as_usage();
                    }
                }
            }

            if usage.used >= usage.limit && usage.limit > 0 {
                let budget = crate::context::context_budget(&self.model);
                let message = format!(
                    "Context full: estimated {} input tokens exceeds the {} token input budget for {} (provider {}, total window {}, display window {}, planned output {}, reserved buffer {}). Run /compact or start a new chat to continue.",
                    usage.used,
                    usage.limit,
                    self.model.meta.id,
                    self.model.provider.id(),
                    budget.total_window,
                    budget.display_window,
                    request_estimate.output_tokens,
                    budget.reserved_buffer
                );
                self.emit(AgentEvent::Error {
                    error: message.clone(),
                })
                .await;
                let cost = total_usage.cost(&self.model.meta.pricing);
                self.emit(AgentEvent::AgentEnd {
                    usage: total_usage,
                    cost,
                    status: RunFinalStatus::Failed {
                        message: message.clone(),
                    },
                })
                .await;
                return Err(crate::error::Error::Llm(imp_llm::Error::ContextTooLong {
                    used: usage.used,
                    limit: usage.limit,
                }));
            }

            // Context management is observation-mask only. Full conversation
            // compaction has been removed because the rewrite-based behavior
            // was too error-prone to keep in the runtime.

            // Build context for the LLM from the exact sanitized request that
            // preflight estimated.
            let context = Context {
                messages: context_messages,
                session_id: self.session_id.clone(),
                thread_id: self.thread_id.clone(),
            };
            self.emit_timing_with_details(
                TimingEvent::new(turn, TimingStage::ContextAssemblyEnd, turn_started_at, None)
                    .with_duration_ms(context_assembly_started_at.elapsed().as_millis() as u64)
                    .with_success(true),
            )
            .await;

            self.hooks.fire(&HookEvent::BeforeLlmCall).await;

            // Pre-flight OAuth token refresh: if we have an auth store and the
            // token is expired, refresh it before making the API call. This
            // avoids wasting a round-trip on a guaranteed 401.
            if let Some(ref auth_store) = self.auth_store {
                let mut store = auth_store.lock().await;
                if store.is_oauth_expired("anthropic") {
                    match store.resolve_with_refresh("anthropic").await {
                        Ok(new_key) => {
                            self.api_key = new_key;
                        }
                        Err(e) => {
                            let message = format!(
                                "OAuth token refresh failed before request: {e}. Continuing with existing credentials."
                            );
                            let _ = self.ui.notify(&message, NotifyLevel::Warning).await;
                        }
                    }
                }
            }

            // Stream the LLM response with retry on transient startup failures.
            turn_state.enter(TurnPhase::SampleModel);
            let llm_request_started_at = Instant::now();
            self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                turn,
                RecoveryCheckpointKind::ProviderRequestStart,
                None,
                None,
                None,
                None,
                None,
            ))
            .await;
            self.emit_timing(
                turn,
                TimingStage::LlmRequestStart,
                turn_started_at,
                Some(llm_request_started_at),
            )
            .await;
            let model = clone_model(&self.model);
            let context = context.clone();
            let request_options = options.clone();
            let api_key = self.api_key.clone();
            let mut stream = crate::retry::stream_with_retry(
                move || {
                    model.provider.stream(
                        &model,
                        context.clone(),
                        request_options.clone(),
                        &api_key,
                    )
                },
                self.retry_policy.clone(),
            );

            let mut ordered_content: Vec<ContentBlock> = Vec::new();
            let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
            let mut assistant_msg: Option<AssistantMessage> = None;
            let mut saw_first_stream_event = false;
            let mut saw_first_text_delta = false;
            let mut saw_first_tool_call = false;
            let mut saw_provider_message_end = false;
            let cancel_token = Arc::clone(&self.cancel_token);
            cancel_token.store(false, std::sync::atomic::Ordering::Relaxed);

            while let Some(event_result) = stream.next().await {
                // Check for cancel during event processing
                while let Ok(cmd) = self.command_rx.try_recv() {
                    match cmd {
                        AgentCommand::Cancel => {
                            cancel_token.store(true, std::sync::atomic::Ordering::Relaxed);
                            cancelled = true;
                            break;
                        }
                        AgentCommand::Steer(msg) => {
                            self.messages.push(Message::user(&msg));
                        }
                        AgentCommand::FollowUp(msg) => queued_follow_ups.push_back(msg),
                    }
                }

                if cancelled {
                    break;
                }

                match event_result {
                    Ok(event) => {
                        if !saw_first_stream_event {
                            saw_first_stream_event = true;
                            self.emit_timing(
                                turn,
                                TimingStage::FirstStreamEvent,
                                turn_started_at,
                                Some(llm_request_started_at),
                            )
                            .await;
                        }
                        // Forward as delta
                        self.emit(AgentEvent::MessageDelta {
                            delta: event.clone(),
                        })
                        .await;

                        match event {
                            StreamEvent::TextDelta { text } => {
                                if !saw_first_text_delta {
                                    saw_first_text_delta = true;
                                    self.emit_timing(
                                        turn,
                                        TimingStage::FirstTextDelta,
                                        turn_started_at,
                                        Some(llm_request_started_at),
                                    )
                                    .await;
                                }
                                push_stream_text_block(&mut ordered_content, text);
                            }
                            StreamEvent::ThinkingDelta { text } => {
                                push_stream_thinking_block(&mut ordered_content, text);
                            }
                            StreamEvent::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                if !saw_first_tool_call {
                                    saw_first_tool_call = true;
                                    self.emit_timing(
                                        turn,
                                        TimingStage::FirstToolCall,
                                        turn_started_at,
                                        Some(llm_request_started_at),
                                    )
                                    .await;
                                }
                                let args_hash = Self::tool_args_hash(&arguments);
                                self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                                    turn,
                                    RecoveryCheckpointKind::AssistantToolCallObserved,
                                    Some(id.clone()),
                                    Some(name.clone()),
                                    Some(args_hash),
                                    None,
                                    None,
                                ))
                                .await;
                                ordered_content.push(ContentBlock::ToolCall {
                                    id: id.clone(),
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                });
                                tool_calls.push((id, name, arguments));
                            }
                            StreamEvent::MessageEnd { message } => {
                                saw_provider_message_end = true;
                                self.emit_timing(
                                    turn,
                                    TimingStage::MessageEnd,
                                    turn_started_at,
                                    Some(llm_request_started_at),
                                )
                                .await;
                                self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                                    turn,
                                    RecoveryCheckpointKind::ProviderRequestCompleted,
                                    None,
                                    None,
                                    None,
                                    Some(true),
                                    None,
                                ))
                                .await;
                                if let Some(ref usage) = message.usage {
                                    total_usage.add(usage);
                                    self.provider_context_baseline_tokens =
                                        Some(usage.total_tokens());
                                }
                                assistant_msg = Some(message);
                            }
                            StreamEvent::MessageStart { .. } => {}
                            StreamEvent::Error { error } => {
                                self.emit(AgentEvent::Error {
                                    error: format!(
                                        "Provider stream failed after partial output: {error}"
                                    ),
                                })
                                .await;
                                // Build a minimal error message to push
                                let err_msg = AssistantMessage {
                                    content: vec![ContentBlock::Text { text: error }],
                                    usage: None,
                                    stop_reason: StopReason::Error("Stream error".to_string()),
                                    timestamp: imp_llm::now(),
                                };
                                self.messages.push(Message::Assistant(err_msg.clone()));
                                turn_state.enter(TurnPhase::RecordObservations);
                                let workflow_review = self.finish_turn_workflow_review(turn);
                                self.emit(AgentEvent::TurnEnd {
                                    index: turn,
                                    message: err_msg,
                                    workflow_review,
                                })
                                .await;
                                let cost = total_usage.cost(&self.model.meta.pricing);
                                self.emit(AgentEvent::AgentEnd {
                                    usage: total_usage,
                                    cost,
                                    status: RunFinalStatus::Failed {
                                        message: "stream error".to_string(),
                                    },
                                })
                                .await;
                                return Err(crate::error::Error::Llm(imp_llm::Error::Provider(
                                    "Stream error".to_string(),
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        let had_partial_output =
                            !ordered_content.is_empty() || !tool_calls.is_empty();
                        let error = match &e {
                            imp_llm::Error::Stream(message) => {
                                if had_partial_output {
                                    format!(
                                        "Provider stream failed after partial output: {message}"
                                    )
                                } else {
                                    format!("Provider stream failed before output: {message}")
                                }
                            }
                            imp_llm::Error::Http(http_error) if http_error.is_decode() => {
                                if had_partial_output {
                                    format!(
                                        "Provider response body decode failed after partial output; not retrying to avoid duplicated tool output: {http_error}"
                                    )
                                } else {
                                    format!(
                                        "Provider response body decode failed before output after retry attempts were exhausted: {http_error}"
                                    )
                                }
                            }
                            _ => {
                                if had_partial_output {
                                    format!("Provider stream failed after partial output: {e}")
                                } else {
                                    e.to_string()
                                }
                            }
                        };
                        self.emit(AgentEvent::Error {
                            error: error.clone(),
                        })
                        .await;
                        if !had_partial_output
                            && recoverable_context_failure(&e)
                            && context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS
                        {
                            let observed_limit = update_observed_input_limit(
                                &mut observed_input_limit,
                                &request_estimate,
                            );
                            self.observed_context_input_limit = observed_input_limit;
                            let effective_display =
                                effective_display_window(&request_estimate, observed_input_limit);
                            self.emit(AgentEvent::ContextUsageUpdated {
                                used: effective_display,
                                display_window: effective_display,
                                input_limit: request_estimate.input_limit,
                                system_tokens: request_estimate.system_tokens,
                                tool_definition_tokens: request_estimate.tool_definition_tokens,
                                message_tokens: request_estimate.message_tokens,
                                output_tokens: request_estimate.output_tokens,
                                observed_input_limit,
                            })
                            .await;
                            if let Some((before, after)) = mask_all_observations_for_recovery(
                                &mut self.messages,
                                &self.model,
                                &options,
                                observed_input_limit,
                                self.provider_context_baseline_tokens,
                            ) {
                                context_recovery_attempts += 1;
                                self.provider_context_baseline_tokens = None;
                                self.emit(AgentEvent::Warning {
                                    message: format!(
                                        "Provider reported context exhaustion before output; {}; treating ~{observed_limit} estimated tokens as the observed input ceiling, masked tool outputs, and retrying ({before} -> {after} estimated tokens).",
                                        format_context_estimate_details(&request_estimate, observed_input_limit)
                                    ),
                                })
                                .await;
                                turn_state
                                    .record_continue(super::ContinueReason::QueuedUserFollowUp);
                                turn += 1;
                                continue 'turns;
                            }
                        }
                        if let Some(follow_up) = recoverable_stream_failure_message(&error) {
                            if stream_recovery_attempts < MAX_STREAM_RECOVERY_ATTEMPTS {
                                stream_recovery_attempts += 1;
                                queued_follow_ups.push_back(follow_up);
                                turn_state
                                    .record_continue(super::ContinueReason::QueuedUserFollowUp);
                                turn += 1;
                                continue 'turns;
                            }
                        }
                        let cost = total_usage.cost(&self.model.meta.pricing);
                        self.emit(AgentEvent::AgentEnd {
                            usage: total_usage,
                            cost,
                            status: RunFinalStatus::Failed {
                                message: error.clone(),
                            },
                        })
                        .await;
                        return Err(e.into());
                    }
                }
            }

            if cancelled {
                // Emit TurnEnd with whatever we have so far
                let partial = assistant_msg.unwrap_or_else(|| {
                    build_assistant_message(&ordered_content, &tool_calls, None)
                });
                self.messages.push(Message::Assistant(partial.clone()));
                let workflow_review = self.finish_turn_workflow_review(turn);
                self.emit(AgentEvent::TurnEnd {
                    index: turn,
                    message: partial,
                    workflow_review,
                })
                .await;
                break;
            }

            // Use the MessageEnd message if provided; otherwise the provider
            // stream ended without a terminal completion event and should be
            // treated as an error rather than silently synthesized as success.
            let msg = match assistant_msg {
                Some(message) => message,
                None if !saw_provider_message_end => {
                    let error = format!(
                        "Provider stream ended unexpectedly before completing the message (missing terminal completion event after {} content block(s) and {} tool call(s))",
                        ordered_content.len(),
                        tool_calls.len()
                    );
                    self.emit(AgentEvent::Error {
                        error: error.clone(),
                    })
                    .await;
                    if let Some(follow_up) = recoverable_stream_failure_message(&error) {
                        if stream_recovery_attempts < MAX_STREAM_RECOVERY_ATTEMPTS {
                            stream_recovery_attempts += 1;
                            queued_follow_ups.push_back(follow_up);
                            turn_state.record_continue(super::ContinueReason::QueuedUserFollowUp);
                            turn += 1;
                            continue;
                        }
                    }
                    let cost = total_usage.cost(&self.model.meta.pricing);
                    self.emit(AgentEvent::AgentEnd {
                        usage: total_usage,
                        cost,
                        status: RunFinalStatus::Failed {
                            message: error.clone(),
                        },
                    })
                    .await;
                    return Err(crate::error::Error::Llm(imp_llm::Error::Stream(error)));
                }
                None => build_assistant_message(&ordered_content, &tool_calls, None),
            };

            if let StopReason::Error(error) = &msg.stop_reason {
                if recoverable_context_failure_message(error)
                    && context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS
                {
                    let observed_limit =
                        update_observed_input_limit(&mut observed_input_limit, &request_estimate);
                    self.observed_context_input_limit = observed_input_limit;
                    let effective_display =
                        effective_display_window(&request_estimate, observed_input_limit);
                    self.emit(AgentEvent::ContextUsageUpdated {
                        used: effective_display,
                        display_window: effective_display,
                        input_limit: request_estimate.input_limit,
                        system_tokens: request_estimate.system_tokens,
                        tool_definition_tokens: request_estimate.tool_definition_tokens,
                        message_tokens: request_estimate.message_tokens,
                        output_tokens: request_estimate.output_tokens,
                        observed_input_limit,
                    })
                    .await;
                    if let Some((before, after)) = mask_all_observations_for_recovery(
                        &mut self.messages,
                        &self.model,
                        &options,
                        observed_input_limit,
                        self.provider_context_baseline_tokens,
                    ) {
                        context_recovery_attempts += 1;
                        self.provider_context_baseline_tokens = None;
                        self.emit(AgentEvent::Warning {
                            message: format!(
                                "Provider reported context exhaustion after completing an error response; {}; treating ~{observed_limit} estimated tokens as the observed input ceiling, masked tool outputs, and retrying ({before} -> {after} estimated tokens).",
                                format_context_estimate_details(&request_estimate, observed_input_limit)
                            ),
                        })
                        .await;
                        turn_state.record_continue(super::ContinueReason::QueuedUserFollowUp);
                        turn += 1;
                        continue 'turns;
                    }
                }
                self.emit(AgentEvent::Error {
                    error: error.clone(),
                })
                .await;
                let cost = total_usage.cost(&self.model.meta.pricing);
                self.emit(AgentEvent::AgentEnd {
                    usage: total_usage,
                    cost,
                    status: RunFinalStatus::Failed {
                        message: error.clone(),
                    },
                })
                .await;
                return Err(crate::error::Error::Llm(imp_llm::Error::Provider(
                    error.clone(),
                )));
            }

            turn_state.enter(TurnPhase::FinalizeAssistantMessage);
            self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                turn,
                RecoveryCheckpointKind::AssistantMessageFinalized,
                None,
                None,
                None,
                Some(true),
                None,
            ))
            .await;
            self.messages.push(Message::Assistant(msg.clone()));

            if tool_calls.is_empty() {
                // No tool calls — the model is done unless a queued follow-up exists.
                let workflow_review = self.finish_turn_workflow_review(turn);
                self.emit(AgentEvent::TurnEnd {
                    index: turn,
                    message: msg.clone(),
                    workflow_review: workflow_review.clone(),
                })
                .await;

                self.emit_timing(
                    turn,
                    TimingStage::PostTurnAssessmentStart,
                    turn_started_at,
                    None,
                )
                .await;
                turn_state.enter(TurnPhase::AssessTurn);
                let assessment_started_at = Instant::now();
                let assessment = self.assess_post_turn(&msg, &[], false, &workflow_review);
                self.emit_timing_with_details(
                    TimingEvent::new(
                        turn,
                        TimingStage::PostTurnAssessmentEnd,
                        turn_started_at,
                        None,
                    )
                    .with_duration_ms(assessment_started_at.elapsed().as_millis() as u64)
                    .with_success(true),
                )
                .await;
                self.emit(AgentEvent::TurnAssessment {
                    index: turn,
                    assessment: assessment.debug_view(),
                })
                .await;
                turn_state.enter(TurnPhase::DecideNext);
                let decision = self.override_finish_with_workflow_decision(
                    self.loop_decision_after_turn(&assessment),
                );
                match decision {
                    LoopDecision::Continue { prompt, reason } => {
                        self.mark_continue_reason(reason);
                        turn_state.record_continue(reason);
                        queued_follow_ups.push_back(prompt);
                    }
                    LoopDecision::Finish { status } => {
                        final_status = Some(status);
                    }
                }

                if let Some(follow_up) = queued_follow_ups.pop_front() {
                    turn_state.record_continue(super::ContinueReason::QueuedUserFollowUp);
                    self.messages.push(Message::user(&follow_up));
                    turn += 1;
                    continue;
                }
                break;
            }

            // Execute tool calls
            turn_state.enter(TurnPhase::PlanTools);
            let tool_plan = self.plan_tools(tool_calls);
            turn_state.record_tool_plan(tool_plan.len());
            for call in &tool_plan.calls {
                self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                    turn,
                    RecoveryCheckpointKind::ToolPlanCreated,
                    Some(call.id.clone()),
                    Some(call.name.clone()),
                    Some(Self::tool_args_hash(&call.args)),
                    Some(call.retry_safe),
                    None,
                ))
                .await;
            }
            turn_state.enter(TurnPhase::ExecuteTools);
            let results = self
                .execute_tool_plan(turn, turn_started_at, tool_plan, cancel_token)
                .await;
            turn_state.record_tool_results(results.len());
            turn_state.enter(TurnPhase::RecordObservations);

            for result in &results {
                self.emit_recovery_checkpoint(Self::recovery_checkpoint(
                    turn,
                    RecoveryCheckpointKind::ToolResultAddedToContext,
                    Some(result.tool_call_id.clone()),
                    Some(result.tool_name.clone()),
                    None,
                    Some(!result.is_error),
                    None,
                ))
                .await;
                self.messages.push(Message::ToolResult(result.clone()));
            }

            self.record_turn_workflow_mutations(&results);
            self.record_obligations_from_tool_results(&results);
            self.record_workflow_obligations_from_tool_results(&results);
            let workflow_review = self.finish_turn_workflow_review(turn);
            self.emit(AgentEvent::TurnEnd {
                index: turn,
                message: msg.clone(),
                workflow_review: workflow_review.clone(),
            })
            .await;

            self.emit_timing(
                turn,
                TimingStage::PostTurnAssessmentStart,
                turn_started_at,
                None,
            )
            .await;
            turn_state.enter(TurnPhase::AssessTurn);
            let assessment_started_at = Instant::now();
            let assessment = self.assess_post_turn(&msg, &results, true, &workflow_review);
            self.emit_timing_with_details(
                TimingEvent::new(
                    turn,
                    TimingStage::PostTurnAssessmentEnd,
                    turn_started_at,
                    None,
                )
                .with_duration_ms(assessment_started_at.elapsed().as_millis() as u64)
                .with_success(true),
            )
            .await;
            self.emit(AgentEvent::TurnAssessment {
                index: turn,
                assessment: assessment.debug_view(),
            })
            .await;
            turn_state.enter(TurnPhase::DecideNext);
            let decision = self
                .override_finish_with_workflow_decision(self.loop_decision_after_turn(&assessment));
            let should_stop_after_tool_turn = matches!(
                decision,
                LoopDecision::Finish {
                    status: RunFinalStatus::Blocked {
                        reason: AgentStopReason::RepeatedAction
                            | AgentStopReason::UserBlocker
                            | AgentStopReason::ExecutionBlocked,
                        ..
                    } | RunFinalStatus::NeedsUserInput { .. }
                        | RunFinalStatus::Cancelled
                        | RunFinalStatus::Failed { .. },
                }
            );
            match decision {
                LoopDecision::Continue { prompt, reason } => {
                    self.mark_continue_reason(reason);
                    turn_state.record_continue(reason);
                    queued_follow_ups.push_back(prompt);
                }
                LoopDecision::Finish { status } => {
                    final_status = Some(status);
                }
            }

            if let Some(follow_up) = queued_follow_ups.pop_front() {
                self.messages.push(Message::user(&follow_up));
                turn_state.record_continue(super::ContinueReason::QueuedUserFollowUp);
                turn += 1;
                continue;
            }
            if should_stop_after_tool_turn {
                break;
            }
            if final_status.is_some() {
                final_status = None;
            }
            turn_state.record_continue(super::ContinueReason::ToolResultsNeedInterpretation);
            turn += 1;
        }

        let mut status = if cancelled {
            RunFinalStatus::Cancelled
        } else {
            final_status.unwrap_or_else(|| {
                RunFinalStatus::from_stop_reason(AgentStopReason::NoAutomaticFollowUp)
            })
        };
        if !cancelled && !self.verification_gates.is_empty() {
            if let Some(artifacts) = &run_artifacts {
                self.run_verification_gates(artifacts).await;
            }
            status = enforce_verification_closeout(status, &self.verification_gates);
        }
        if !cancelled {
            status = self.enforce_workflow_closeout_status(status);
        }
        let worktree_metadata = if let Some(artifacts) = &run_artifacts {
            self.capture_worktree_run_artifacts(artifacts).await
        } else {
            None
        };
        if let Some(artifacts) = &run_artifacts {
            self.write_run_evidence(
                &run_id,
                artifacts,
                worktree_metadata.as_ref(),
                &prompt,
                &status,
            )
            .await;
            self.write_closeout_eval_candidate(&run_id, artifacts, &prompt, &status)
                .await;
            self.persist_workflow_controller_snapshot(Some(artifacts))
                .await;
        }
        let cost = total_usage.cost(&self.model.meta.pricing);
        self.emit(AgentEvent::AgentEnd {
            usage: total_usage,
            cost,
            status: status.clone(),
        })
        .await;

        if let Ok(mut active_trace_writer) = self.trace_writer.lock() {
            *active_trace_writer = None;
        }
        if let Ok(mut active_run_id) = self.run_id.lock() {
            *active_run_id = None;
        }

        if cancelled {
            return Err(crate::error::Error::Cancelled);
        }

        Ok(())
    }
}
