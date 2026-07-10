use std::path::PathBuf;

use imp_llm::{AssistantMessage, ContentBlock, Cost, Message, StreamEvent, Usage};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::reference_monitor::{PolicyTraceRecord, ToolPolicyDecision};
use crate::runtime::{
    RuntimeArtifactRef, RuntimeAssistantBlock, RuntimeAssistantDelta, RuntimeContextUsage,
    RuntimeEvent, RuntimeEventKind, RuntimeMessageRole, RuntimePolicyDecision,
    RuntimePolicyDecisionKind, RuntimePolicyWarningKind, RuntimeRecoverySummary, RuntimeToolCall,
    RuntimeToolStatus, RuntimeTranscriptMessage, RuntimeUsageSummary, RuntimeVerificationUpdate,
    RuntimeWorktreeNotice, RuntimeWorktreeNoticeKind, RuntimeWorktreeState,
};
use crate::trace::TraceEvent;
use crate::trust::Provenance;
use crate::workflow::VerificationGate;
use crate::workflow_review::TurnWorkflowReview;

use super::NextActionAssessment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingStage {
    ContextAssemblyStart,
    ContextAssemblyEnd,
    LlmRequestStart,
    FirstStreamEvent,
    FirstTextDelta,
    FirstToolCall,
    MessageEnd,
    ToolExecutionStart,
    ToolExecutionEnd,
    PostTurnAssessmentStart,
    PostTurnAssessmentEnd,
}

impl TimingStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContextAssemblyStart => "context_assembly_start",
            Self::ContextAssemblyEnd => "context_assembly_end",
            Self::LlmRequestStart => "llm_request_start",
            Self::FirstStreamEvent => "first_stream_event",
            Self::FirstTextDelta => "first_text_delta",
            Self::FirstToolCall => "first_tool_call",
            Self::MessageEnd => "message_end",
            Self::ToolExecutionStart => "tool_execution_start",
            Self::ToolExecutionEnd => "tool_execution_end",
            Self::PostTurnAssessmentStart => "post_turn_assessment_start",
            Self::PostTurnAssessmentEnd => "post_turn_assessment_end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingEvent {
    pub turn: u32,
    pub stage: TimingStage,
    pub since_turn_start_ms: u64,
    pub since_llm_request_start_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub label: Option<String>,
    pub success: Option<bool>,
}

impl TimingEvent {
    pub fn new(
        turn: u32,
        stage: TimingStage,
        turn_started_at: std::time::Instant,
        llm_request_started_at: Option<std::time::Instant>,
    ) -> Self {
        let now = std::time::Instant::now();
        Self {
            turn,
            stage,
            since_turn_start_ms: now.duration_since(turn_started_at).as_millis() as u64,
            since_llm_request_start_ms: llm_request_started_at
                .map(|started_at| now.duration_since(started_at).as_millis() as u64),
            duration_ms: None,
            label: None,
            success: None,
        }
    }

    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_success(mut self, success: bool) -> Self {
        self.success = Some(success);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCheckpointKind {
    ProviderRequestStart,
    AssistantToolCallObserved,
    AssistantMessageFinalized,
    ToolPlanCreated,
    ToolExecutionStart,
    ToolExecutionEnd,
    ToolResultAddedToContext,
    ProviderRequestCompleted,
}

impl RecoveryCheckpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderRequestStart => "provider_request_start",
            Self::AssistantToolCallObserved => "assistant_tool_call_observed",
            Self::AssistantMessageFinalized => "assistant_message_finalized",
            Self::ToolPlanCreated => "tool_plan_created",
            Self::ToolExecutionStart => "tool_execution_start",
            Self::ToolExecutionEnd => "tool_execution_end",
            Self::ToolResultAddedToContext => "tool_result_added_to_context",
            Self::ProviderRequestCompleted => "provider_request_completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCheckpoint {
    pub version: u32,
    pub turn: u32,
    pub kind: RecoveryCheckpointKind,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub args_hash: Option<String>,
    pub success: Option<bool>,
    pub error_class: Option<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEventKind {
    SessionStarted,
    Navigated,
    Observation,
    InputRequested,
    InputApproved,
    InputDenied,
    ActionCompleted,
    SessionFailed,
    SessionStopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserEvent {
    pub kind: BrowserEventKind,
    pub session_id: Option<String>,
    pub engine: String,
    pub action: Option<String>,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub title: Option<String>,
    pub approval_scope: Option<String>,
    pub outcome: Option<String>,
    pub duration_ms: Option<u64>,
    pub sequence: Option<u64>,
    pub interactive_elements: Option<u32>,
}

impl BrowserEvent {
    pub fn new(kind: BrowserEventKind) -> Self {
        Self {
            kind,
            session_id: None,
            engine: "lightpanda".into(),
            action: None,
            url: None,
            domain: None,
            title: None,
            approval_scope: None,
            outcome: None,
            duration_ms: None,
            sequence: None,
            interactive_elements: None,
        }
    }
}

/// Events emitted by the agent during execution.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart {
        model: String,
        timestamp: u64,
    },
    AgentEnd {
        usage: Usage,
        cost: Cost,
        status: crate::agent::RunFinalStatus,
    },
    TurnStart {
        index: u32,
    },
    TurnAssessment {
        index: u32,
        assessment: NextActionAssessment,
    },
    TurnEnd {
        index: u32,
        message: AssistantMessage,
        workflow_review: TurnWorkflowReview,
    },
    MessageStart {
        message: Message,
    },
    MessageDelta {
        delta: StreamEvent,
    },
    MessageEnd {
        message: Message,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolOutputDelta {
        tool_call_id: String,
        text: String,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        result: imp_llm::ToolResultMessage,
        provenance: Option<Provenance>,
    },
    Browser {
        event: BrowserEvent,
    },
    ContextUsageUpdated {
        used: u32,
        display_window: u32,
        input_limit: u32,
        system_tokens: u32,
        tool_definition_tokens: u32,
        message_tokens: u32,
        output_tokens: u32,
        observed_input_limit: Option<u32>,
    },
    Warning {
        message: String,
    },
    Timing {
        timing: TimingEvent,
    },
    RecoveryCheckpoint {
        checkpoint: RecoveryCheckpoint,
    },
    WorkflowControllerSnapshot {
        snapshot: crate::workflow::WorkflowControllerSnapshot,
    },
    VerificationStarted {
        gate: VerificationGate,
    },
    VerificationCompleted {
        gate: VerificationGate,
        closeout_effect: crate::workflow::VerificationCloseoutEffect,
    },
    WorktreeCreated {
        metadata: crate::workflow::WorktreeRunMetadata,
    },
    WorktreeDiffCaptured {
        metadata: crate::workflow::WorktreeRunMetadata,
    },
    WorktreeCloseout {
        result: crate::workflow::WorktreeCloseoutResult,
    },
    EvidenceWritten {
        path: PathBuf,
    },
    PolicyChecked {
        record: PolicyTraceRecord,
    },
    Error {
        error: String,
    },
}

impl AgentEvent {
    pub fn to_runtime_event(&self, run_id: impl Into<String>, sequence: u64) -> RuntimeEvent {
        RuntimeEvent {
            run_id: run_id.into(),
            sequence,
            timestamp_ms: self.source_timestamp_ms(),
            kind: self.to_runtime_event_kind(),
            ..RuntimeEvent::default()
        }
    }

    fn source_timestamp_ms(&self) -> Option<u64> {
        match self {
            AgentEvent::AgentStart { timestamp, .. } => Some(*timestamp),
            AgentEvent::RecoveryCheckpoint { checkpoint } => Some(checkpoint.timestamp),
            _ => None,
        }
    }

    fn to_runtime_event_kind(&self) -> RuntimeEventKind {
        match self {
            AgentEvent::AgentStart { model, .. } => RuntimeEventKind::AgentStarted {
                model: model.clone(),
            },
            AgentEvent::AgentEnd { usage, cost, status } => RuntimeEventKind::AgentEnded {
                status: status.clone().into(),
                usage: Some(runtime_usage_summary(usage, cost)),
            },
            AgentEvent::TurnStart { index } => RuntimeEventKind::TurnStarted { index: *index },
            AgentEvent::TurnAssessment { index, assessment } => RuntimeEventKind::TurnAssessed {
                index: *index,
                summary: Some(format!("{assessment:?}")),
            },
            AgentEvent::TurnEnd { index, message, .. } => RuntimeEventKind::TurnCompleted {
                index: *index,
                message: runtime_assistant_message(message, false),
                usage: message.usage.as_ref().map(runtime_usage_without_cost),
            },
            AgentEvent::MessageStart { message } => RuntimeEventKind::MessageObserved {
                message: runtime_message(message, true),
            },
            AgentEvent::MessageDelta { delta } => match delta {
                StreamEvent::TextDelta { text } => RuntimeEventKind::AssistantDelta {
                    delta: RuntimeAssistantDelta::VisibleText { text: text.clone() },
                },
                StreamEvent::ThinkingDelta { text } => RuntimeEventKind::AssistantDelta {
                    delta: RuntimeAssistantDelta::Thinking { text: text.clone() },
                },
                StreamEvent::ToolCall { id, name, arguments } => RuntimeEventKind::ToolDeclared {
                    tool_call: runtime_tool_call(id, name, arguments, RuntimeToolStatus::Pending),
                },
                StreamEvent::Error { error } => RuntimeEventKind::Error {
                    message: error.clone(),
                },
                StreamEvent::MessageStart { model } => RuntimeEventKind::AgentStarted {
                    model: model.clone(),
                },
                StreamEvent::MessageEnd { message } => RuntimeEventKind::MessageFinalized {
                    message: runtime_assistant_message(message, false),
                },
            },
            AgentEvent::MessageEnd { message } => RuntimeEventKind::MessageFinalized {
                message: runtime_message(message, false),
            },
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                RuntimeEventKind::ToolStarted {
                    tool_call: runtime_tool_call(
                        tool_call_id,
                        tool_name,
                        args,
                        RuntimeToolStatus::Running,
                    ),
                }
            }
            AgentEvent::ToolOutputDelta { tool_call_id, text } => RuntimeEventKind::ToolOutput {
                tool_call_id: tool_call_id.clone(),
                output_delta: text.clone(),
            },
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                provenance,
            } => RuntimeEventKind::ToolCompleted {
                tool_call: RuntimeToolCall {
                    id: tool_call_id.clone(),
                    name: result.tool_name.clone(),
                    status: if result.is_error {
                        RuntimeToolStatus::Failed
                    } else {
                        RuntimeToolStatus::Succeeded
                    },
                    output_preview: tool_result_summary(result),
                    details: Some(redact_runtime_value(&result.details)),
                    warning: provenance.as_ref().and_then(runtime_provenance_warning),
                    is_error: result.is_error,
                    ..RuntimeToolCall::default()
                },
            },
            AgentEvent::Browser { event } => RuntimeEventKind::BrowserUpdated {
                event: event.clone(),
            },
            AgentEvent::ContextUsageUpdated {
                used,
                display_window,
                input_limit,
                system_tokens,
                tool_definition_tokens,
                message_tokens,
                output_tokens,
                observed_input_limit,
            } => RuntimeEventKind::ContextUsageUpdated {
                usage: RuntimeContextUsage {
                    used: *used,
                    display_window: *display_window,
                    input_limit: *input_limit,
                    system_tokens: *system_tokens,
                    tool_definition_tokens: *tool_definition_tokens,
                    message_tokens: *message_tokens,
                    output_tokens: *output_tokens,
                    observed_input_limit: *observed_input_limit,
                },
            },
            AgentEvent::Warning { message } => RuntimeEventKind::Warning {
                message: message.clone(),
            },
            AgentEvent::Timing { timing } => RuntimeEventKind::Timing {
                stage: timing.stage.as_str().into(),
                duration_ms: timing.duration_ms,
                success: timing.success,
            },
            AgentEvent::RecoveryCheckpoint { checkpoint } => RuntimeEventKind::RecoveryUpdated {
                recovery: RuntimeRecoverySummary {
                    kind: checkpoint.kind.as_str().into(),
                    turn: checkpoint.turn,
                    tool_call_id: checkpoint.tool_call_id.clone(),
                    tool_name: checkpoint.tool_name.clone(),
                    success: checkpoint.success,
                    error_class: checkpoint.error_class.clone(),
                },
            },
            AgentEvent::WorkflowControllerSnapshot { snapshot } => {
                RuntimeEventKind::WorkflowControllerUpdated { snapshot: snapshot.clone() }
            }
            AgentEvent::VerificationStarted { gate } => {
                let mut gate = gate.clone();
                gate.mark_running();
                RuntimeEventKind::VerificationUpdated { gate }
            }
            AgentEvent::VerificationCompleted { gate, closeout_effect } => {
                RuntimeEventKind::VerificationCompleted {
                    update: RuntimeVerificationUpdate {
                        gate: gate.clone(),
                        closeout_effect: Some(*closeout_effect),
                    },
                }
            }
            AgentEvent::WorktreeCreated { metadata } => runtime_worktree_notice(
                RuntimeWorktreeNoticeKind::Created,
                format!(
                    "Worktree-auto active: editing {} on branch {} (original checkout: {}).",
                    metadata.worktree_path.display(),
                    metadata.branch,
                    metadata.main_worktree.display()
                ),
                RuntimeWorktreeState {
                    metadata: metadata.clone(),
                    ..RuntimeWorktreeState::default()
                },
            ),
            AgentEvent::WorktreeDiffCaptured { metadata } => runtime_worktree_notice(
                RuntimeWorktreeNoticeKind::DiffCaptured,
                format!(
                    "Worktree diff captured: {}. Closeout choices: keep worktree, apply patch, or discard worktree.",
                    metadata.patch_path.display()
                ),
                RuntimeWorktreeState {
                    metadata: metadata.clone(),
                    ..RuntimeWorktreeState::default()
                },
            ),
            AgentEvent::WorktreeCloseout { result } => runtime_worktree_notice(
                RuntimeWorktreeNoticeKind::Closeout,
                format!("Worktree closeout: {}", result.message),
                RuntimeWorktreeState {
                    closeout: Some(result.clone()),
                    ..RuntimeWorktreeState::default()
                },
            ),
            AgentEvent::EvidenceWritten { path } => RuntimeEventKind::EvidenceUpdated {
                artifact: RuntimeArtifactRef {
                    kind: "evidence-packet".into(),
                    path: path.clone(),
                    summary: Some("Run evidence packet".into()),
                },
            },
            AgentEvent::PolicyChecked { record } => RuntimeEventKind::PolicyDecision {
                decision: runtime_policy_decision(record),
            },
            AgentEvent::Error { error } => RuntimeEventKind::Error {
                message: error.clone(),
            },
        }
    }

    pub fn to_trace_event(&self, run_id: impl Into<String>) -> TraceEvent {
        let run_id = run_id.into();
        match self {
            AgentEvent::AgentStart { model, timestamp } => TraceEvent::new(
                run_id,
                "agent.start",
                json!({ "model": model, "source_timestamp": timestamp }),
            ),
            AgentEvent::AgentEnd {
                usage,
                cost,
                status,
            } => TraceEvent::new(
                run_id,
                "agent.end",
                json!({
                    "usage": format!("{usage:?}"),
                    "cost": format!("{cost:?}"),
                    "status": format!("{status:?}"),
                }),
            ),
            AgentEvent::TurnStart { index } => {
                TraceEvent::new(run_id, "turn.start", json!({ "index": index })).with_turn(*index)
            }
            AgentEvent::TurnAssessment { index, assessment } => TraceEvent::new(
                run_id,
                "turn.assessment",
                json!({ "index": index, "assessment": format!("{assessment:?}") }),
            )
            .with_turn(*index),
            AgentEvent::TurnEnd {
                index,
                message,
                workflow_review,
            } => TraceEvent::new(
                run_id,
                "turn.end",
                json!({
                    "index": index,
                    "message": format!("{message:?}"),
                    "workflow_review": format!("{workflow_review:?}"),
                }),
            )
            .with_turn(*index),
            AgentEvent::MessageStart { message } => TraceEvent::new(
                run_id,
                "message.start",
                json!({ "message": format!("{message:?}") }),
            ),
            AgentEvent::MessageDelta { delta } => TraceEvent::new(
                run_id,
                "message.delta",
                json!({ "delta": format!("{delta:?}") }),
            ),
            AgentEvent::MessageEnd { message } => TraceEvent::new(
                run_id,
                "message.end",
                json!({ "message": format!("{message:?}") }),
            ),
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => TraceEvent::new(
                run_id,
                "tool.execution.start",
                json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "args": args }),
            )
            .with_tool_call_id(tool_call_id.clone()),
            AgentEvent::ToolOutputDelta { tool_call_id, text } => TraceEvent::new(
                run_id,
                "tool.output.delta",
                json!({ "tool_call_id": tool_call_id, "text": text }),
            )
            .with_tool_call_id(tool_call_id.clone()),
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                provenance,
            } => TraceEvent::new(
                run_id,
                "tool.execution.end",
                json!({
                    "tool_call_id": tool_call_id,
                    "result": format!("{result:?}"),
                    "provenance": provenance,
                }),
            )
            .with_tool_call_id(tool_call_id.clone()),
            AgentEvent::Browser { event } => TraceEvent::new(
                run_id,
                browser_trace_kind(event.kind),
                serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
            ),
            AgentEvent::ContextUsageUpdated {
                used,
                display_window,
                input_limit,
                system_tokens,
                tool_definition_tokens,
                message_tokens,
                output_tokens,
                observed_input_limit,
            } => TraceEvent::new(
                run_id,
                "context.usage",
                json!({
                    "used": used,
                    "display_window": display_window,
                    "input_limit": input_limit,
                    "system_tokens": system_tokens,
                    "tool_definition_tokens": tool_definition_tokens,
                    "message_tokens": message_tokens,
                    "output_tokens": output_tokens,
                    "observed_input_limit": observed_input_limit,
                }),
            ),
            AgentEvent::Warning { message } => {
                TraceEvent::new(run_id, "warning", json!({ "message": message }))
            }
            AgentEvent::Timing { timing } => TraceEvent::new(
                run_id,
                "timing",
                json!({
                    "turn": timing.turn,
                    "stage": timing.stage.as_str(),
                    "since_turn_start_ms": timing.since_turn_start_ms,
                    "since_llm_request_start_ms": timing.since_llm_request_start_ms,
                    "duration_ms": timing.duration_ms,
                    "label": timing.label,
                    "success": timing.success,
                }),
            )
            .with_turn(timing.turn),
            AgentEvent::RecoveryCheckpoint { checkpoint } => {
                let mut event = TraceEvent::new(
                    run_id,
                    "recovery.checkpoint",
                    json!({
                        "version": checkpoint.version,
                        "turn": checkpoint.turn,
                        "kind": checkpoint.kind.as_str(),
                        "tool_call_id": checkpoint.tool_call_id,
                        "tool_name": checkpoint.tool_name,
                        "args_hash": checkpoint.args_hash,
                        "success": checkpoint.success,
                        "error_class": checkpoint.error_class,
                        "checkpoint_timestamp": checkpoint.timestamp,
                    }),
                )
                .with_turn(checkpoint.turn);
                if let Some(tool_call_id) = &checkpoint.tool_call_id {
                    event = event.with_tool_call_id(tool_call_id.clone());
                }
                event
            }
            AgentEvent::WorkflowControllerSnapshot { snapshot } => TraceEvent::new(
                run_id,
                "workflow.controller",
                json!({ "snapshot": snapshot }),
            ),
            AgentEvent::VerificationStarted { gate } => TraceEvent::new(
                run_id,
                "verification.started",
                verification_gate_payload(gate, None),
            ),
            AgentEvent::VerificationCompleted {
                gate,
                closeout_effect,
            } => TraceEvent::new(
                run_id,
                "verification.completed",
                verification_gate_payload(gate, Some(*closeout_effect)),
            ),
            AgentEvent::WorktreeCreated { metadata } => TraceEvent::new(
                run_id,
                "worktree.created",
                worktree_metadata_payload(metadata),
            ),
            AgentEvent::WorktreeDiffCaptured { metadata } => TraceEvent::new(
                run_id,
                "worktree.diff_captured",
                worktree_metadata_payload(metadata),
            ),
            AgentEvent::WorktreeCloseout { result } => TraceEvent::new(
                run_id,
                "worktree.closeout",
                json!({
                    "action": result.action,
                    "applied": result.applied,
                    "removed": result.removed,
                    "branch_deleted": result.branch_deleted,
                    "clean": result.clean,
                    "message": result.message,
                }),
            ),
            AgentEvent::EvidenceWritten { path } => TraceEvent::new(
                run_id,
                "evidence.written",
                json!({ "path": path.display().to_string() }),
            ),
            AgentEvent::PolicyChecked { record } => record.to_trace_event(run_id),
            AgentEvent::Error { error } => {
                TraceEvent::new(run_id, "error", json!({ "error": error }))
            }
        }
    }
}

fn browser_trace_kind(kind: BrowserEventKind) -> &'static str {
    match kind {
        BrowserEventKind::SessionStarted => "browser.session.started",
        BrowserEventKind::Navigated => "browser.navigated",
        BrowserEventKind::Observation => "browser.observation",
        BrowserEventKind::InputRequested => "browser.input.requested",
        BrowserEventKind::InputApproved => "browser.input.approved",
        BrowserEventKind::InputDenied => "browser.input.denied",
        BrowserEventKind::ActionCompleted => "browser.action.completed",
        BrowserEventKind::SessionFailed => "browser.session.failed",
        BrowserEventKind::SessionStopped => "browser.session.stopped",
    }
}

fn runtime_usage_summary(usage: &Usage, cost: &Cost) -> RuntimeUsageSummary {
    RuntimeUsageSummary {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        raw_total_tokens: usage.raw_total_tokens(),
        effective_total_tokens: usage.effective_total_tokens(),
        total_tokens: usage.total_tokens(),
        input_cost_micros: cost_micros(cost.input),
        output_cost_micros: cost_micros(cost.output),
        cache_read_cost_micros: cost_micros(cost.cache_read),
        cache_write_cost_micros: cost_micros(cost.cache_write),
        total_cost_micros: cost_micros(cost.total),
        total_cost: Some(format!("{:.6}", cost.total)),
    }
}

fn cost_micros(cost: f64) -> u64 {
    if cost.is_finite() && cost > 0.0 {
        (cost * 1_000_000.0).round() as u64
    } else {
        0
    }
}

fn content_text(content: &[ContentBlock]) -> Option<String> {
    let text = content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } | ContentBlock::Thinking { text } => text.as_str(),
            ContentBlock::ToolCall { name, .. } => name.as_str(),
            ContentBlock::Image { .. } => "[image]",
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn tool_result_summary(result: &imp_llm::ToolResultMessage) -> Option<String> {
    content_text(&result.content)
}

fn runtime_usage_without_cost(usage: &Usage) -> RuntimeUsageSummary {
    runtime_usage_summary(usage, &Cost::default())
}

fn runtime_message(message: &Message, streaming: bool) -> RuntimeTranscriptMessage {
    match message {
        Message::User(message) => RuntimeTranscriptMessage {
            id: format!("user-{}", message.timestamp),
            role: RuntimeMessageRole::User,
            blocks: runtime_content_blocks(&message.content),
            timestamp_ms: Some(message.timestamp.saturating_mul(1000)),
            ..RuntimeTranscriptMessage::default()
        },
        Message::Assistant(message) => runtime_assistant_message(message, streaming),
        Message::ToolResult(result) => RuntimeTranscriptMessage {
            id: format!("tool-result-{}-{}", result.tool_call_id, result.timestamp),
            role: RuntimeMessageRole::ToolResult,
            blocks: runtime_content_blocks(&result.content),
            timestamp_ms: Some(result.timestamp.saturating_mul(1000)),
            ..RuntimeTranscriptMessage::default()
        },
    }
}

fn runtime_assistant_message(
    message: &AssistantMessage,
    streaming: bool,
) -> RuntimeTranscriptMessage {
    RuntimeTranscriptMessage {
        id: format!("assistant-{}", message.timestamp),
        role: RuntimeMessageRole::Assistant,
        blocks: runtime_content_blocks(&message.content),
        is_streaming: streaming,
        timestamp_ms: Some(message.timestamp.saturating_mul(1000)),
        ..RuntimeTranscriptMessage::default()
    }
}

fn runtime_content_blocks(content: &[ContentBlock]) -> Vec<RuntimeAssistantBlock> {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => {
                Some(RuntimeAssistantBlock::VisibleText { text: text.clone() })
            }
            ContentBlock::Thinking { text } => {
                Some(RuntimeAssistantBlock::Thinking { text: text.clone() })
            }
            ContentBlock::ToolCall { id, .. } => Some(RuntimeAssistantBlock::ToolCall {
                tool_call_id: id.clone(),
            }),
            ContentBlock::Image { .. } => None,
        })
        .collect()
}

fn runtime_tool_call(
    id: &str,
    name: &str,
    arguments: &serde_json::Value,
    status: RuntimeToolStatus,
) -> RuntimeToolCall {
    let arguments = redact_runtime_value(arguments);
    RuntimeToolCall {
        id: id.to_string(),
        name: name.to_string(),
        status,
        args_preview: Some(arguments.to_string()),
        arguments: Some(arguments),
        ..RuntimeToolCall::default()
    }
}

pub(crate) fn redact_runtime_value(value: &serde_json::Value) -> serde_json::Value {
    const SENSITIVE: &[&str] = &[
        "api_key",
        "authorization",
        "cookie",
        "password",
        "secret",
        "token",
        "value",
    ];
    match value {
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let redacted = if SENSITIVE
                        .iter()
                        .any(|sensitive| key.to_ascii_lowercase().contains(sensitive))
                    {
                        serde_json::Value::String("[redacted]".into())
                    } else {
                        redact_runtime_value(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(redact_runtime_value).collect())
        }
        other => other.clone(),
    }
}

fn runtime_provenance_warning(provenance: &Provenance) -> Option<String> {
    use crate::trust::{RiskLabel, TrustLabel};

    if provenance.trust == TrustLabel::ExternalUntrusted
        || provenance
            .risk
            .contains(&RiskLabel::PossiblePromptInjection)
        || provenance.risk.contains(&RiskLabel::ContainsInstructions)
    {
        Some(format!(
            "Trust warning: low-trust content observed from {} cannot authorize policy/tool escalation.",
            provenance.origin.as_deref().unwrap_or("unknown source")
        ))
    } else {
        None
    }
}

fn runtime_worktree_notice(
    kind: RuntimeWorktreeNoticeKind,
    message: String,
    worktree: RuntimeWorktreeState,
) -> RuntimeEventKind {
    RuntimeEventKind::WorktreeNotice {
        notice: RuntimeWorktreeNotice {
            kind,
            message,
            worktree,
        },
    }
}

fn runtime_policy_decision(record: &PolicyTraceRecord) -> RuntimePolicyDecision {
    let (decision, reason) = match &record.decision {
        ToolPolicyDecision::Allow { reasons } => (
            RuntimePolicyDecisionKind::Allow,
            reasons.first().map(|reason| reason.message.clone()),
        ),
        ToolPolicyDecision::Deny { reason } => (
            RuntimePolicyDecisionKind::Deny,
            Some(reason.message.clone()),
        ),
        ToolPolicyDecision::AskUser { reason }
        | ToolPolicyDecision::DryRunOnly { reason }
        | ToolPolicyDecision::SandboxOnly { reason }
        | ToolPolicyDecision::RequireVerification { reason } => (
            RuntimePolicyDecisionKind::Warn,
            Some(reason.message.clone()),
        ),
    };
    let warning = runtime_policy_warning(record);
    RuntimePolicyDecision {
        id: record.tool_call_id.clone(),
        subject: record.tool_name.clone(),
        decision,
        reason,
        warning: warning.as_ref().map(|(_, message)| message.clone()),
        warning_kind: warning.map(|(kind, _)| kind),
    }
}

fn runtime_policy_warning(
    record: &PolicyTraceRecord,
) -> Option<(RuntimePolicyWarningKind, String)> {
    let reason = match &record.decision {
        ToolPolicyDecision::Allow { reasons } => reasons.iter().find(|reason| {
            matches!(
                reason.source,
                crate::reference_monitor::PolicySource::TrustLabel
                    | crate::reference_monitor::PolicySource::ToolManifest
                    | crate::reference_monitor::PolicySource::ConfigPolicy
            )
        })?,
        ToolPolicyDecision::Deny { reason }
        | ToolPolicyDecision::AskUser { reason }
        | ToolPolicyDecision::DryRunOnly { reason }
        | ToolPolicyDecision::SandboxOnly { reason }
        | ToolPolicyDecision::RequireVerification { reason } => reason,
    };
    let (kind, prefix) = match reason.source {
        crate::reference_monitor::PolicySource::TrustLabel => {
            (RuntimePolicyWarningKind::Trust, "Trust warning")
        }
        crate::reference_monitor::PolicySource::ToolManifest
        | crate::reference_monitor::PolicySource::ConfigPolicy => {
            (RuntimePolicyWarningKind::Extension, "Extension policy")
        }
        _ => (RuntimePolicyWarningKind::Policy, "Policy warning"),
    };
    Some((
        kind,
        format!("{prefix}: {} ({})", reason.message, reason.code),
    ))
}

fn worktree_metadata_payload(metadata: &crate::workflow::WorktreeRunMetadata) -> serde_json::Value {
    json!({
        "repo_root": metadata.repo_root.display().to_string(),
        "main_worktree": metadata.main_worktree.display().to_string(),
        "worktree_path": metadata.worktree_path.display().to_string(),
        "branch": metadata.branch,
        "start_point": metadata.start_point,
        "run_id": metadata.run_id,
        "workflow_id": metadata.workflow_id,
        "status_path": metadata.status_path.display().to_string(),
        "stat_path": metadata.stat_path.display().to_string(),
        "patch_path": metadata.patch_path.display().to_string(),
        "clean": metadata.clean,
    })
}

fn verification_gate_payload(
    gate: &VerificationGate,
    closeout_effect: Option<crate::workflow::VerificationCloseoutEffect>,
) -> serde_json::Value {
    json!({
        "id": gate.id,
        "name": gate.name,
        "kind": gate.kind,
        "required": gate.is_required(),
        "status": gate.status,
        "command": gate.command.as_ref().map(|command| &command.command),
        "exit_code": gate.result.as_ref().and_then(|result| result.exit_code),
        "summary": gate.result.as_ref().and_then(|result| result.summary.as_deref()).or(gate.reason.as_deref()),
        "artifacts": gate.artifacts.iter().map(|artifact| json!({
            "kind": artifact.kind,
            "path": artifact.path.display().to_string(),
            "summary": artifact.summary,
            "bytes": artifact.bytes,
            "redaction": artifact.redaction,
        })).collect::<Vec<_>>(),
        "closeout_effect": closeout_effect,
    })
}

#[cfg(test)]
mod trace_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_events_convert_to_trace_events() {
        let start = AgentEvent::AgentStart {
            model: "test-model".into(),
            timestamp: 123,
        }
        .to_trace_event("run-1");
        assert_eq!(start.kind, "agent.start");
        assert_eq!(start.run_id, "run-1");
        assert_eq!(start.payload["model"], "test-model");

        let tool = AgentEvent::ToolExecutionStart {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            args: json!({"path": "README.md"}),
        }
        .to_trace_event("run-1");
        assert_eq!(tool.kind, "tool.execution.start");
        assert_eq!(tool.correlation.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(tool.payload["tool_name"], "read");

        let timing = AgentEvent::Timing {
            timing: TimingEvent {
                turn: 2,
                stage: TimingStage::LlmRequestStart,
                since_turn_start_ms: 10,
                since_llm_request_start_ms: None,
                duration_ms: None,
                label: None,
                success: None,
            },
        }
        .to_trace_event("run-1");
        assert_eq!(timing.kind, "timing");
        assert_eq!(timing.turn, Some(2));
        assert_eq!(timing.payload["stage"], "llm_request_start");
        let worktree = AgentEvent::WorktreeDiffCaptured {
            metadata: crate::workflow::WorktreeRunMetadata {
                repo_root: "/repo".into(),
                main_worktree: "/repo".into(),
                worktree_path: "/tmp/worktree".into(),
                branch: "imp/run/worktree-auto".into(),
                run_id: "run-1".into(),
                patch_path: "/repo/.imp/runs/run-1/worktree/diff.patch".into(),
                clean: false,
                ..crate::workflow::WorktreeRunMetadata::default()
            },
        }
        .to_trace_event("run-1");
        assert_eq!(worktree.kind, "worktree.diff_captured");
        assert_eq!(worktree.payload["worktree_path"], "/tmp/worktree");
        assert_eq!(worktree.payload["branch"], "imp/run/worktree-auto");
        assert_eq!(
            worktree.payload["patch_path"],
            "/repo/.imp/runs/run-1/worktree/diff.patch"
        );
        assert_eq!(worktree.payload["clean"], false);

        let closeout = AgentEvent::WorktreeCloseout {
            result: crate::workflow::WorktreeCloseoutResult {
                action: crate::workflow::WorktreeCloseoutAction::Keep,
                message: "kept".into(),
                ..crate::workflow::WorktreeCloseoutResult::default()
            },
        }
        .to_trace_event("run-1");
        assert_eq!(closeout.kind, "worktree.closeout");
        assert_eq!(closeout.payload["action"], "keep");
    }

    #[test]
    fn agent_events_convert_to_runtime_events() {
        let runtime = AgentEvent::ToolExecutionStart {
            tool_call_id: "tool-1".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({ "command": "cargo test" }),
        }
        .to_runtime_event("run-1", 42);
        assert_eq!(runtime.run_id, "run-1");
        assert_eq!(runtime.sequence, 42);
        match runtime.kind {
            RuntimeEventKind::ToolStarted { tool_call } => {
                assert_eq!(tool_call.id, "tool-1");
                assert_eq!(tool_call.name, "bash");
                assert_eq!(tool_call.status, RuntimeToolStatus::Running);
                assert!(tool_call
                    .args_preview
                    .as_deref()
                    .is_some_and(|args| args.contains("cargo test")));
            }
            other => panic!("unexpected runtime event kind: {other:?}"),
        }

        let policy = AgentEvent::PolicyChecked {
            record: PolicyTraceRecord {
                run_id: None,
                workflow_id: None,
                turn: None,
                tool_call_id: Some("tool-1".into()),
                tool_name: "bash".into(),
                action_kind: crate::reference_monitor::ToolActionKind::Execute,
                decision: ToolPolicyDecision::Deny {
                    reason: crate::reference_monitor::PolicyReason::new(
                        crate::reference_monitor::PolicySource::Guardrail,
                        "deny_test",
                        "blocked for test",
                    ),
                },
                args_hash: None,
                resource_scope: crate::reference_monitor::ResourceScope::Command {
                    program: "bash".into(),
                },
                workflow_type: crate::workflow::WorkflowType::AdHoc,
                risk_level: crate::workflow::RiskLevel::Low,
                trust_scope: crate::reference_monitor::TrustScopeContext::default(),
                trust_labels: Vec::new(),
                details: serde_json::Value::Null,
            },
        }
        .to_runtime_event("run-1", 43);
        match policy.kind {
            RuntimeEventKind::PolicyDecision { decision } => {
                assert_eq!(decision.id.as_deref(), Some("tool-1"));
                assert_eq!(decision.subject, "bash");
                assert_eq!(decision.decision, RuntimePolicyDecisionKind::Deny);
                assert_eq!(decision.reason.as_deref(), Some("blocked for test"));
            }
            other => panic!("unexpected runtime event kind: {other:?}"),
        }
    }

    #[test]
    fn recovery_checkpoint_conversion_preserves_correlation() {
        let event = AgentEvent::RecoveryCheckpoint {
            checkpoint: RecoveryCheckpoint {
                version: 1,
                turn: 3,
                kind: RecoveryCheckpointKind::ToolExecutionEnd,
                tool_call_id: Some("call-2".into()),
                tool_name: Some("bash".into()),
                args_hash: Some("abc".into()),
                success: Some(false),
                error_class: Some("timeout".into()),
                timestamp: 456,
            },
        }
        .to_trace_event("run-1");

        assert_eq!(event.kind, "recovery.checkpoint");
        assert_eq!(event.turn, Some(3));
        assert_eq!(event.correlation.tool_call_id.as_deref(), Some("call-2"));
        assert_eq!(event.payload["kind"], "tool_execution_end");
        assert_eq!(event.payload["error_class"], "timeout");
    }
}

#[cfg(test)]
mod runtime_conversion_tests {
    use super::*;
    use crate::runtime::{RuntimeAssistantDelta, RuntimeEventKind};

    #[test]
    fn runtime_conversion_distinguishes_visible_text_and_thinking() {
        let visible = AgentEvent::MessageDelta {
            delta: StreamEvent::TextDelta {
                text: "hello".into(),
            },
        }
        .to_runtime_event("run", 1);
        let thinking = AgentEvent::MessageDelta {
            delta: StreamEvent::ThinkingDelta { text: "hmm".into() },
        }
        .to_runtime_event("run", 2);

        assert!(matches!(
            visible.kind,
            RuntimeEventKind::AssistantDelta {
                delta: RuntimeAssistantDelta::VisibleText { ref text }
            } if text == "hello"
        ));
        assert!(matches!(
            thinking.kind,
            RuntimeEventKind::AssistantDelta {
                delta: RuntimeAssistantDelta::Thinking { ref text }
            } if text == "hmm"
        ));
    }

    #[test]
    fn runtime_tool_arguments_are_recursively_redacted() {
        let event = AgentEvent::ToolExecutionStart {
            tool_call_id: "tool".into(),
            tool_name: "web".into(),
            args: serde_json::json!({
                "query": "safe",
                "headers": {
                    "authorization": "Bearer secret",
                    "nested_token": "secret"
                }
            }),
        }
        .to_runtime_event("run", 1);

        let RuntimeEventKind::ToolStarted { tool_call } = event.kind else {
            panic!("expected tool start");
        };
        let arguments = tool_call.arguments.expect("redacted arguments");
        assert_eq!(arguments["query"], "safe");
        assert_eq!(arguments["headers"]["authorization"], "[redacted]");
        assert_eq!(arguments["headers"]["nested_token"], "[redacted]");
        assert!(!serde_json::to_string(&arguments)
            .unwrap()
            .contains("Bearer secret"));
    }
}
