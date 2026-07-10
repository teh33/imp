use super::{
    RuntimeAssistantBlock, RuntimeAssistantDelta, RuntimeEvent, RuntimeEventKind,
    RuntimeFinalStatus, RuntimeMessageRole, RuntimePhase, RuntimeStateDelta, RuntimeStateSnapshot,
    RuntimeToolCall, RuntimeToolStatus, RuntimeTranscriptMessage, RuntimeTurn, RuntimeTurnStatus,
    RuntimeUsageSummary, MAX_RUNTIME_ERROR_COUNT, MAX_RUNTIME_TOOL_OUTPUT_CHARS,
    MAX_RUNTIME_WARNING_COUNT, RUNTIME_SCHEMA_VERSION,
};
use crate::workflow::ChildWorkflowStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateAccumulator {
    snapshot: RuntimeStateSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeApplyOutcome {
    Applied(RuntimeStateDelta),
    Gap {
        expected: u64,
        received: u64,
        delta: RuntimeStateDelta,
    },
    Duplicate {
        sequence: u64,
    },
    Stale {
        last: u64,
        received: u64,
    },
    RunMismatch {
        expected: String,
        received: String,
    },
    EmptyRunId,
    UnsupportedSchema {
        supported: u32,
        received: u32,
    },
}

impl RuntimeStateAccumulator {
    pub fn new(run_id: impl Into<String>) -> Self {
        let mut snapshot = RuntimeStateSnapshot::default();
        let run_id = run_id.into();
        if !run_id.is_empty() {
            snapshot.workflow.run_id = Some(run_id);
        }
        Self { snapshot }
    }

    pub fn from_snapshot(snapshot: RuntimeStateSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn reset(&mut self, run_id: impl Into<String>) {
        *self = Self::new(run_id);
    }

    pub fn begin_run(&mut self, run_id: impl Into<String>) {
        let transcript = std::mem::take(&mut self.snapshot.transcript);
        let completed_tools = std::mem::take(&mut self.snapshot.completed_tools);
        *self = Self::new(run_id);
        self.snapshot.transcript = transcript;
        self.snapshot.completed_tools = completed_tools;
    }

    pub fn apply(&mut self, event: &RuntimeEvent) -> RuntimeApplyOutcome {
        if event.schema_version != RUNTIME_SCHEMA_VERSION {
            return RuntimeApplyOutcome::UnsupportedSchema {
                supported: RUNTIME_SCHEMA_VERSION,
                received: event.schema_version,
            };
        }
        if event.run_id.is_empty() {
            return RuntimeApplyOutcome::EmptyRunId;
        }
        if let Some(expected) = self.snapshot.workflow.run_id.as_deref() {
            if expected != event.run_id {
                return RuntimeApplyOutcome::RunMismatch {
                    expected: expected.to_string(),
                    received: event.run_id.clone(),
                };
            }
        } else {
            self.snapshot.workflow.run_id = Some(event.run_id.clone());
        }
        if let Some(last) = self.snapshot.last_sequence {
            if event.sequence == last {
                return RuntimeApplyOutcome::Duplicate {
                    sequence: event.sequence,
                };
            }
            if event.sequence < last {
                return RuntimeApplyOutcome::Stale {
                    last,
                    received: event.sequence,
                };
            }
        }

        let expected = self
            .snapshot
            .last_sequence
            .map_or(event.sequence, |last| last + 1);
        let is_gap = self.snapshot.last_sequence.is_some() && event.sequence > expected;
        let mut delta = RuntimeStateDelta {
            sequence: event.sequence,
            ..RuntimeStateDelta::default()
        };
        self.reduce(event, &mut delta);
        self.snapshot.last_sequence = Some(event.sequence);
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
        delta.revision = self.snapshot.revision;

        if is_gap {
            push_bounded(
                &mut self.snapshot.warnings,
                format!(
                    "runtime sequence gap: expected {expected}, received {}",
                    event.sequence
                ),
                MAX_RUNTIME_WARNING_COUNT,
            );
            delta.diagnostics_changed = true;
            RuntimeApplyOutcome::Gap {
                expected,
                received: event.sequence,
                delta,
            }
        } else {
            RuntimeApplyOutcome::Applied(delta)
        }
    }

    pub fn snapshot(&self) -> RuntimeStateSnapshot {
        self.snapshot.clone()
    }

    pub fn snapshot_ref(&self) -> &RuntimeStateSnapshot {
        &self.snapshot
    }

    fn reduce(&mut self, event: &RuntimeEvent, delta: &mut RuntimeStateDelta) {
        match &event.kind {
            RuntimeEventKind::AgentStarted { model } => self.start_agent(model, delta),
            RuntimeEventKind::AgentEnded { status, usage } => self.end_agent(status, usage, delta),
            RuntimeEventKind::TurnStarted { index } => self.start_turn(*index, delta),
            RuntimeEventKind::TurnAssessed { index, summary } => {
                let turn = upsert_turn(&mut self.snapshot.turns, *index);
                turn.assessment = summary.clone();
                delta.workflow_changed = true;
            }
            RuntimeEventKind::TurnEnded { index } => self.end_turn(*index, None, None, delta),
            RuntimeEventKind::TurnCompleted {
                index,
                message,
                usage,
            } => self.end_turn(*index, Some(message), usage.as_ref(), delta),
            RuntimeEventKind::MessageStarted { role, summary } => {
                self.start_message(role, summary.as_deref(), event, delta)
            }
            RuntimeEventKind::MessageObserved { message } => self.observe_message(message, delta),
            RuntimeEventKind::MessageDelta { delta: text } => self.append_assistant_delta(
                RuntimeAssistantDelta::VisibleText { text: text.clone() },
                event,
                delta,
            ),
            RuntimeEventKind::AssistantDelta { delta: content } => {
                self.append_assistant_delta(content.clone(), event, delta)
            }
            RuntimeEventKind::MessageEnded { .. } => self.finalize_active_message(None, delta),
            RuntimeEventKind::MessageFinalized { message } => {
                self.finalize_active_message(Some(message), delta)
            }
            RuntimeEventKind::SessionHydrated {
                transcript,
                completed_tools,
            } => {
                self.snapshot.transcript = transcript.clone();
                self.snapshot.completed_tools = completed_tools.clone();
                self.snapshot.active_message_id = transcript
                    .iter()
                    .rev()
                    .find(|message| message.is_streaming)
                    .map(|message| message.id.clone());
                delta.transcript_changed = true;
                delta.tools_changed = !completed_tools.is_empty();
            }
            RuntimeEventKind::ToolDeclared { tool_call } => {
                self.declare_tool(tool_call, event, delta)
            }
            RuntimeEventKind::ToolStarted { tool_call } => self.start_tool(tool_call, delta),
            RuntimeEventKind::ToolOutput {
                tool_call_id,
                output_delta,
            } => self.append_tool_output(tool_call_id, output_delta, delta),
            RuntimeEventKind::ToolCompleted { tool_call } => self.complete_tool(tool_call, delta),
            RuntimeEventKind::BrowserUpdated { event } => self.update_browser(event, delta),
            RuntimeEventKind::ApprovalPending { approval } => {
                self.snapshot.phase = RuntimePhase::WaitingForApproval;
                upsert_by(
                    &mut self.snapshot.pending_approvals,
                    approval.clone(),
                    |item| &item.id,
                );
                delta.phase_changed = true;
                delta.approvals_changed = true;
            }
            RuntimeEventKind::ApprovalResolved { approval } => {
                self.snapshot
                    .pending_approvals
                    .retain(|item| item.id != approval.id);
                upsert_by(
                    &mut self.snapshot.resolved_approvals,
                    approval.clone(),
                    |item| &item.id,
                );
                delta.approvals_changed = true;
            }
            RuntimeEventKind::PolicyDecision { decision } => {
                self.snapshot.policy_decisions.push(decision.clone());
                if decision.decision == super::RuntimePolicyDecisionKind::Deny {
                    self.snapshot.phase = RuntimePhase::Blocked;
                    delta.phase_changed = true;
                }
                if let Some(warning) = &decision.warning {
                    push_bounded(
                        &mut self.snapshot.warnings,
                        warning.clone(),
                        MAX_RUNTIME_WARNING_COUNT,
                    );
                    delta.diagnostics_changed = true;
                }
                delta.policy_changed = true;
            }
            RuntimeEventKind::WorkflowControllerUpdated { snapshot } => {
                self.snapshot.workflow.controller = Some(snapshot.clone());
                delta.workflow_changed = true;
            }
            RuntimeEventKind::VerificationUpdated { gate } => {
                self.snapshot.phase = RuntimePhase::Verifying;
                upsert_by(
                    &mut self.snapshot.verification_gates,
                    gate.clone(),
                    |item| &item.id,
                );
                delta.phase_changed = true;
                delta.verification_changed = true;
            }
            RuntimeEventKind::VerificationCompleted { update } => {
                upsert_by(
                    &mut self.snapshot.verification_gates,
                    update.gate.clone(),
                    |item| &item.id,
                );
                upsert_by(&mut self.snapshot.verification, update.clone(), |item| {
                    &item.gate.id
                });
                delta.verification_changed = true;
            }
            RuntimeEventKind::EvidenceUpdated { artifact } => {
                upsert_artifact(&mut self.snapshot.evidence_refs, artifact.clone());
                self.snapshot
                    .status_items
                    .insert("evidence".into(), artifact.path.display().to_string());
                delta.evidence_changed = true;
            }
            RuntimeEventKind::ChildWorkflowUpdated { child } => {
                upsert_by(&mut self.snapshot.child_workflows, child.clone(), |item| {
                    &item.id
                });
                self.snapshot.phase = runtime_phase_for_child_status(child.status);
                self.snapshot.status_items.insert(
                    "child-workflow".into(),
                    format!("{}:{:?}", child.id, child.status),
                );
                for artifact in &child.evidence_refs {
                    upsert_artifact(&mut self.snapshot.evidence_refs, artifact.clone());
                }
                delta.phase_changed = true;
                delta.workflow_changed = true;
                delta.evidence_changed = true;
            }
            RuntimeEventKind::WorktreeUpdated { worktree } => self.update_worktree(worktree, delta),
            RuntimeEventKind::WorktreeNotice { notice } => {
                self.update_worktree(&notice.worktree, delta);
                let (key, value) = match notice.kind {
                    super::RuntimeWorktreeNoticeKind::Created => (
                        "worktree",
                        format!(
                            "{} @ {}",
                            notice.worktree.metadata.branch,
                            notice.worktree.metadata.worktree_path.display()
                        ),
                    ),
                    super::RuntimeWorktreeNoticeKind::DiffCaptured => (
                        "worktree-diff",
                        notice.worktree.metadata.patch_path.display().to_string(),
                    ),
                    super::RuntimeWorktreeNoticeKind::Closeout => {
                        ("worktree-closeout", notice.message.clone())
                    }
                };
                self.snapshot.status_items.insert(key.into(), value);
            }
            RuntimeEventKind::ManaUpdated { workflow_ref } => {
                upsert_by(
                    &mut self.snapshot.workflow_refs,
                    workflow_ref.clone(),
                    |item| &item.id,
                );
                delta.workflow_changed = true;
            }
            RuntimeEventKind::ContextUsageUpdated { usage } => {
                self.snapshot.context_usage = usage.clone();
                delta.usage_changed = true;
            }
            RuntimeEventKind::Warning { message } => {
                push_bounded(
                    &mut self.snapshot.warnings,
                    message.clone(),
                    MAX_RUNTIME_WARNING_COUNT,
                );
                delta.diagnostics_changed = true;
            }
            RuntimeEventKind::Error { message } => self.record_error(message, event, delta),
            RuntimeEventKind::Timing { stage, .. } => {
                self.snapshot
                    .status_items
                    .insert("timing".into(), stage.clone());
                delta.workflow_changed = true;
            }
            RuntimeEventKind::RecoveryCheckpoint {
                kind,
                turn,
                tool_call_id,
            } => {
                self.snapshot.recovery.push(super::RuntimeRecoverySummary {
                    kind: kind.clone(),
                    turn: *turn,
                    tool_call_id: tool_call_id.clone(),
                    ..super::RuntimeRecoverySummary::default()
                });
                delta.workflow_changed = true;
            }
            RuntimeEventKind::RecoveryUpdated { recovery } => {
                self.snapshot.recovery.push(recovery.clone());
                delta.workflow_changed = true;
            }
            RuntimeEventKind::Unknown { name } => {
                self.snapshot
                    .status_items
                    .insert("last-unknown-event".into(), name.clone());
                delta.workflow_changed = true;
            }
        }
    }

    fn start_agent(&mut self, model: &str, delta: &mut RuntimeStateDelta) {
        self.snapshot.phase = RuntimePhase::Running;
        self.snapshot.workflow.model = Some(model.to_string());
        self.snapshot.final_status = None;
        self.snapshot
            .status_items
            .insert("model".into(), model.into());
        delta.phase_changed = true;
        delta.workflow_changed = true;
        delta.final_status_changed = true;
    }

    fn end_agent(
        &mut self,
        status: &RuntimeFinalStatus,
        usage: &Option<RuntimeUsageSummary>,
        delta: &mut RuntimeStateDelta,
    ) {
        self.snapshot.final_status = Some(status.clone());
        self.snapshot.phase = runtime_phase_for_final_status(status);
        let phase = match self.snapshot.phase {
            RuntimePhase::Completed => "completed",
            RuntimePhase::Blocked => "blocked",
            RuntimePhase::Failed => "failed",
            _ => "ended",
        };
        self.snapshot
            .status_items
            .insert("phase".into(), phase.into());
        if let Some(usage) = usage {
            let turn_usage = self.snapshot.usage.clone();
            let context_tokens = turn_usage.raw_total_tokens;
            let mut final_usage = usage.clone();
            if turn_usage.input_tokens > 0 || turn_usage.output_tokens > 0 {
                final_usage.input_tokens = turn_usage.input_tokens;
                final_usage.output_tokens = turn_usage.output_tokens;
                final_usage.cache_read_tokens = turn_usage.cache_read_tokens;
                final_usage.cache_write_tokens = turn_usage.cache_write_tokens;
                final_usage.effective_total_tokens = turn_usage.effective_total_tokens;
                final_usage.total_tokens = turn_usage.total_tokens;
            }
            if context_tokens > 0 {
                final_usage.raw_total_tokens = context_tokens;
            }
            self.snapshot.usage = final_usage;
            self.snapshot
                .status_items
                .insert("tokens".into(), usage.total_tokens.to_string());
            if let Some(cost) = &usage.total_cost {
                self.snapshot
                    .status_items
                    .insert("cost".into(), cost.clone());
            }
            delta.usage_changed = true;
        }
        self.finalize_active_message(None, delta);
        delta.phase_changed = true;
        delta.final_status_changed = true;
    }

    fn start_turn(&mut self, index: u32, delta: &mut RuntimeStateDelta) {
        self.snapshot.phase = RuntimePhase::Running;
        self.snapshot
            .status_items
            .insert("turn".into(), index.to_string());
        let turn = upsert_turn(&mut self.snapshot.turns, index);
        turn.status = RuntimeTurnStatus::Running;
        delta.phase_changed = true;
        delta.workflow_changed = true;
    }

    fn end_turn(
        &mut self,
        index: u32,
        message: Option<&RuntimeTranscriptMessage>,
        usage: Option<&RuntimeUsageSummary>,
        delta: &mut RuntimeStateDelta,
    ) {
        let turn = upsert_turn(&mut self.snapshot.turns, index);
        turn.status = RuntimeTurnStatus::Completed;
        if let Some(message) = message {
            self.finalize_active_message(Some(message), delta);
        }
        if let Some(usage) = usage {
            add_usage(&mut self.snapshot.usage, usage);
            delta.usage_changed = true;
        }
        delta.workflow_changed = true;
    }

    fn start_message(
        &mut self,
        role: &str,
        summary: Option<&str>,
        event: &RuntimeEvent,
        delta: &mut RuntimeStateDelta,
    ) {
        let role = parse_role(role);
        let id = format!("message-{}", event.sequence);
        let mut message = RuntimeTranscriptMessage {
            id: id.clone(),
            role,
            is_streaming: role == RuntimeMessageRole::Assistant,
            timestamp_ms: event.timestamp_ms,
            ..RuntimeTranscriptMessage::default()
        };
        if let Some(summary) = summary.filter(|summary| !summary.is_empty()) {
            message.blocks.push(RuntimeAssistantBlock::VisibleText {
                text: summary.into(),
            });
        }
        if message.is_streaming {
            self.snapshot.active_message_id = Some(id.clone());
        }
        self.snapshot.transcript.push(message);
        delta.transcript_changed = true;
        delta.changed_message_id = Some(id);
    }

    fn observe_message(
        &mut self,
        message: &RuntimeTranscriptMessage,
        delta: &mut RuntimeStateDelta,
    ) {
        if let Some(existing) = self
            .snapshot
            .transcript
            .iter_mut()
            .find(|item| item.id == message.id)
        {
            *existing = message.clone();
        } else {
            self.snapshot.transcript.push(message.clone());
        }
        if message.is_streaming {
            self.snapshot.active_message_id = Some(message.id.clone());
        }
        delta.transcript_changed = true;
        delta.changed_message_id = Some(message.id.clone());
    }

    fn append_assistant_delta(
        &mut self,
        content: RuntimeAssistantDelta,
        event: &RuntimeEvent,
        delta: &mut RuntimeStateDelta,
    ) {
        let id = self.ensure_active_assistant(event);
        if let Some(message) = self
            .snapshot
            .transcript
            .iter_mut()
            .find(|item| item.id == id)
        {
            let block = match content {
                RuntimeAssistantDelta::VisibleText { text } => {
                    RuntimeAssistantBlock::VisibleText { text }
                }
                RuntimeAssistantDelta::Thinking { text } => {
                    RuntimeAssistantBlock::Thinking { text }
                }
            };
            append_message_block(message, block);
        }
        delta.transcript_changed = true;
        delta.changed_message_id = Some(id);
    }

    fn ensure_active_assistant(&mut self, event: &RuntimeEvent) -> String {
        if let Some(id) = &self.snapshot.active_message_id {
            return id.clone();
        }
        let id = format!("assistant-{}", event.sequence);
        self.snapshot.transcript.push(RuntimeTranscriptMessage {
            id: id.clone(),
            role: RuntimeMessageRole::Assistant,
            is_streaming: true,
            timestamp_ms: event.timestamp_ms,
            ..RuntimeTranscriptMessage::default()
        });
        self.snapshot.active_message_id = Some(id.clone());
        id
    }

    fn finalize_active_message(
        &mut self,
        replacement: Option<&RuntimeTranscriptMessage>,
        delta: &mut RuntimeStateDelta,
    ) {
        let Some(id) = self.snapshot.active_message_id.take() else {
            if let Some(replacement) = replacement {
                self.observe_message(replacement, delta);
            }
            return;
        };
        if let Some(message) = self
            .snapshot
            .transcript
            .iter_mut()
            .find(|item| item.id == id)
        {
            if let Some(replacement) = replacement {
                let mut replacement = replacement.clone();
                replacement.id = id.clone();
                replacement.is_streaming = false;
                *message = replacement;
            } else {
                message.is_streaming = false;
            }
        }
        delta.transcript_changed = true;
        delta.changed_message_id = Some(id);
    }

    fn declare_tool(
        &mut self,
        tool_call: &RuntimeToolCall,
        event: &RuntimeEvent,
        delta: &mut RuntimeStateDelta,
    ) {
        let mut tool_call = tool_call.clone();
        tool_call.status = RuntimeToolStatus::Pending;
        upsert_tool(&mut self.snapshot.active_tools, tool_call.clone());
        let message_id = self.ensure_active_assistant(event);
        if let Some(message) = self
            .snapshot
            .transcript
            .iter_mut()
            .find(|item| item.id == message_id)
        {
            if !message.blocks.iter().any(|block| matches!(block, RuntimeAssistantBlock::ToolCall { tool_call_id } if tool_call_id == &tool_call.id)) {
                message.blocks.push(RuntimeAssistantBlock::ToolCall { tool_call_id: tool_call.id.clone() });
            }
        }
        delta.transcript_changed = true;
        delta.tools_changed = true;
        delta.changed_message_id = Some(message_id);
        delta.changed_tool_call_id = Some(tool_call.id);
    }

    fn start_tool(&mut self, tool_call: &RuntimeToolCall, delta: &mut RuntimeStateDelta) {
        self.snapshot.phase = RuntimePhase::WaitingForTool;
        let mut tool_call = tool_call.clone();
        tool_call.status = RuntimeToolStatus::Running;
        upsert_tool(&mut self.snapshot.active_tools, tool_call.clone());
        delta.phase_changed = true;
        delta.tools_changed = true;
        delta.changed_tool_call_id = Some(tool_call.id);
    }

    fn append_tool_output(&mut self, id: &str, output: &str, delta: &mut RuntimeStateDelta) {
        if let Some(tool) = self
            .snapshot
            .active_tools
            .iter_mut()
            .find(|tool| tool.id == id)
        {
            let combined = format!(
                "{}{}",
                tool.output_preview.as_deref().unwrap_or_default(),
                output
            );
            tool.output_preview = Some(bounded_tail(&combined, MAX_RUNTIME_TOOL_OUTPUT_CHARS));
        }
        delta.tools_changed = true;
        delta.changed_tool_call_id = Some(id.to_string());
    }

    fn complete_tool(&mut self, tool_call: &RuntimeToolCall, delta: &mut RuntimeStateDelta) {
        let mut completed = tool_call.clone();
        let active = self
            .snapshot
            .active_tools
            .iter()
            .find(|tool| tool.id == tool_call.id);
        if completed.arguments.is_none() {
            completed.arguments = active.and_then(|tool| tool.arguments.clone());
        }
        if completed.args_preview.is_none() {
            completed.args_preview = active.and_then(|tool| tool.args_preview.clone());
        }
        let details = if tool_call.details.is_some() {
            tool_call.details.clone()
        } else {
            self.snapshot
                .active_tools
                .iter()
                .find(|tool| tool.id == tool_call.id)
                .and_then(|tool| tool.details.clone())
        };
        completed.details = details;
        if completed.output_preview.is_none() {
            completed.output_preview = self
                .snapshot
                .active_tools
                .iter()
                .find(|tool| tool.id == completed.id)
                .and_then(|tool| tool.output_preview.clone());
        }
        self.snapshot
            .active_tools
            .retain(|tool| tool.id != completed.id);
        upsert_tool(&mut self.snapshot.completed_tools, completed.clone());
        self.snapshot.phase = RuntimePhase::Running;
        delta.phase_changed = true;
        delta.tools_changed = true;
        delta.changed_tool_call_id = Some(completed.id);
    }

    fn update_worktree(
        &mut self,
        worktree: &super::RuntimeWorktreeState,
        delta: &mut RuntimeStateDelta,
    ) {
        if !worktree.metadata.worktree_path.as_os_str().is_empty() {
            self.snapshot.workspace.scope = crate::workflow::WorkspaceScope::Worktree {
                path: worktree.metadata.worktree_path.clone(),
                branch: Some(worktree.metadata.branch.clone()),
            };
        }
        if let Some(existing) = &mut self.snapshot.workspace.worktree {
            if !worktree.metadata.worktree_path.as_os_str().is_empty() {
                existing.metadata = worktree.metadata.clone();
            }
            if worktree.closeout.is_some() {
                existing.closeout = worktree.closeout.clone();
            }
        } else {
            self.snapshot.workspace.worktree = Some(worktree.clone());
        }
        delta.workflow_changed = true;
    }

    fn update_browser(
        &mut self,
        event: &crate::agent::BrowserEvent,
        delta: &mut RuntimeStateDelta,
    ) {
        use crate::agent::BrowserEventKind;

        self.snapshot.status_items.insert(
            "browser".into(),
            format!(
                "{:?}:{}",
                event.kind,
                event.domain.as_deref().unwrap_or("local")
            ),
        );
        let approval = browser_approval_ref(event);
        match event.kind {
            BrowserEventKind::InputRequested => {
                self.snapshot.phase = RuntimePhase::WaitingForApproval;
                upsert_by(&mut self.snapshot.pending_approvals, approval, |item| {
                    &item.id
                });
                delta.approvals_changed = true;
                delta.phase_changed = true;
            }
            BrowserEventKind::InputApproved | BrowserEventKind::InputDenied => {
                self.snapshot
                    .pending_approvals
                    .retain(|pending| pending.id != approval.id);
                upsert_by(&mut self.snapshot.resolved_approvals, approval, |item| {
                    &item.id
                });
                self.snapshot.phase = RuntimePhase::Running;
                delta.approvals_changed = true;
                delta.phase_changed = true;
            }
            _ => {}
        }
        delta.workflow_changed = true;
    }

    fn record_error(&mut self, message: &str, event: &RuntimeEvent, delta: &mut RuntimeStateDelta) {
        push_bounded(
            &mut self.snapshot.errors,
            message.to_string(),
            MAX_RUNTIME_ERROR_COUNT,
        );
        self.snapshot.phase = RuntimePhase::Failed;
        let active_id = self.snapshot.active_message_id.clone().or_else(|| {
            self.snapshot
                .transcript
                .iter()
                .rev()
                .find(|message| {
                    message.role == RuntimeMessageRole::Assistant && message.is_streaming
                })
                .map(|message| message.id.clone())
        });
        let active_has_output = active_id.as_deref().is_some_and(|id| {
            self.snapshot.transcript.iter().any(|item| {
                item.id == id
                    && item.blocks.iter().any(|block| match block {
                        RuntimeAssistantBlock::VisibleText { text }
                        | RuntimeAssistantBlock::Thinking { text } => !text.trim().is_empty(),
                        RuntimeAssistantBlock::ToolCall { .. } => true,
                    })
            })
        });
        if active_has_output {
            if let Some(existing) = self
                .snapshot
                .transcript
                .iter_mut()
                .find(|item| Some(item.id.as_str()) == active_id.as_deref())
            {
                existing.is_streaming = false;
            }
        }
        let replacement_id = (!active_has_output).then_some(active_id.clone()).flatten();
        let error_message = RuntimeTranscriptMessage {
            id: replacement_id
                .clone()
                .unwrap_or_else(|| format!("error-{}", event.sequence)),
            role: RuntimeMessageRole::Error,
            blocks: vec![RuntimeAssistantBlock::VisibleText {
                text: message.to_string(),
            }],
            is_streaming: false,
            timestamp_ms: event.timestamp_ms,
            error_replacement: replacement_id.is_some(),
        };
        if let Some(id) = replacement_id {
            if let Some(existing) = self
                .snapshot
                .transcript
                .iter_mut()
                .find(|item| item.id == id)
            {
                *existing = error_message.clone();
            }
        } else {
            self.snapshot.transcript.push(error_message.clone());
        }
        self.snapshot.active_message_id = None;
        delta.phase_changed = true;
        delta.diagnostics_changed = true;
        delta.transcript_changed = true;
        delta.changed_message_id = Some(error_message.id);
    }
}

fn browser_approval_ref(event: &crate::agent::BrowserEvent) -> super::RuntimeApprovalRef {
    use crate::agent::BrowserEventKind;

    let session = event.session_id.as_deref().unwrap_or("browser");
    let action = event.action.as_deref().unwrap_or("input");
    super::RuntimeApprovalRef {
        id: format!("{session}:{action}"),
        summary: format!(
            "browser {action} on {}",
            event.domain.as_deref().unwrap_or("current page")
        ),
        status: match event.kind {
            BrowserEventKind::InputApproved => super::RuntimeApprovalStatus::Approved,
            BrowserEventKind::InputDenied => super::RuntimeApprovalStatus::Denied,
            _ => super::RuntimeApprovalStatus::Pending,
        },
        requested_by: Some("browser".into()),
    }
}

fn parse_role(role: &str) -> RuntimeMessageRole {
    match role {
        "user" => RuntimeMessageRole::User,
        "tool_result" => RuntimeMessageRole::ToolResult,
        "system" => RuntimeMessageRole::System,
        "warning" => RuntimeMessageRole::Warning,
        "error" => RuntimeMessageRole::Error,
        _ => RuntimeMessageRole::Assistant,
    }
}

fn append_message_block(message: &mut RuntimeTranscriptMessage, block: RuntimeAssistantBlock) {
    match (message.blocks.last_mut(), block) {
        (
            Some(RuntimeAssistantBlock::VisibleText { text }),
            RuntimeAssistantBlock::VisibleText { text: next },
        )
        | (
            Some(RuntimeAssistantBlock::Thinking { text }),
            RuntimeAssistantBlock::Thinking { text: next },
        ) => text.push_str(&next),
        (_, block) => message.blocks.push(block),
    }
}

fn add_usage(total: &mut RuntimeUsageSummary, usage: &RuntimeUsageSummary) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    total.raw_total_tokens = total
        .raw_total_tokens
        .saturating_add(usage.raw_total_tokens);
    total.effective_total_tokens = total
        .effective_total_tokens
        .saturating_add(usage.effective_total_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    total.input_cost_micros = total
        .input_cost_micros
        .saturating_add(usage.input_cost_micros);
    total.output_cost_micros = total
        .output_cost_micros
        .saturating_add(usage.output_cost_micros);
    total.cache_read_cost_micros = total
        .cache_read_cost_micros
        .saturating_add(usage.cache_read_cost_micros);
    total.cache_write_cost_micros = total
        .cache_write_cost_micros
        .saturating_add(usage.cache_write_cost_micros);
    total.total_cost_micros = total
        .total_cost_micros
        .saturating_add(usage.total_cost_micros);
    total.total_cost = Some(format!(
        "{:.6}",
        total.total_cost_micros as f64 / 1_000_000.0
    ));
}

fn upsert_turn(turns: &mut Vec<RuntimeTurn>, index: u32) -> &mut RuntimeTurn {
    if let Some(position) = turns.iter().position(|turn| turn.index == index) {
        return &mut turns[position];
    }
    turns.push(RuntimeTurn {
        index,
        ..RuntimeTurn::default()
    });
    turns.last_mut().expect("turn just pushed")
}

fn upsert_tool(tools: &mut Vec<RuntimeToolCall>, tool_call: RuntimeToolCall) {
    if let Some(existing) = tools.iter_mut().find(|tool| tool.id == tool_call.id) {
        let output = existing.output_preview.clone();
        *existing = tool_call;
        if existing.output_preview.is_none() {
            existing.output_preview = output;
        }
    } else {
        tools.push(tool_call);
    }
}

fn upsert_by<T, K: PartialEq>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> &K) {
    if let Some(existing) = items
        .iter_mut()
        .find(|existing| key(existing) == key(&item))
    {
        *existing = item;
    } else {
        items.push(item);
    }
}

fn upsert_artifact(items: &mut Vec<super::RuntimeArtifactRef>, item: super::RuntimeArtifactRef) {
    if let Some(existing) = items
        .iter_mut()
        .find(|existing| existing.kind == item.kind && existing.path == item.path)
    {
        *existing = item;
    } else {
        items.push(item);
    }
}

fn push_bounded<T>(items: &mut Vec<T>, item: T, max: usize) {
    items.push(item);
    if items.len() > max {
        items.drain(..items.len() - max);
    }
}

fn bounded_tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail = text.chars().skip(count - max_chars).collect::<String>();
    format!("…{tail}")
}

fn runtime_phase_for_final_status(status: &RuntimeFinalStatus) -> RuntimePhase {
    match status {
        RuntimeFinalStatus::Done | RuntimeFinalStatus::DoneWithConcerns { .. } => {
            RuntimePhase::Completed
        }
        RuntimeFinalStatus::Blocked { .. } | RuntimeFinalStatus::NeedsContext { .. } => {
            RuntimePhase::Blocked
        }
        RuntimeFinalStatus::Cancelled | RuntimeFinalStatus::Failed { .. } => RuntimePhase::Failed,
    }
}

fn runtime_phase_for_child_status(status: ChildWorkflowStatus) -> RuntimePhase {
    match status {
        ChildWorkflowStatus::Planned
        | ChildWorkflowStatus::Queued
        | ChildWorkflowStatus::Starting => RuntimePhase::Starting,
        ChildWorkflowStatus::Running => RuntimePhase::Running,
        ChildWorkflowStatus::WaitingForApproval => RuntimePhase::WaitingForApproval,
        ChildWorkflowStatus::WaitingForTool => RuntimePhase::WaitingForTool,
        ChildWorkflowStatus::WaitingForParent
        | ChildWorkflowStatus::Blocked
        | ChildWorkflowStatus::Stale => RuntimePhase::Blocked,
        ChildWorkflowStatus::Cancelling
        | ChildWorkflowStatus::Cancelled
        | ChildWorkflowStatus::Failed => RuntimePhase::Failed,
        ChildWorkflowStatus::Done
        | ChildWorkflowStatus::DoneWithConcerns
        | ChildWorkflowStatus::Integrated => RuntimePhase::Completed,
    }
}
