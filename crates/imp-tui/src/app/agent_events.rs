use imp_core::agent::{AgentEvent, RunFinalStatus};
use imp_core::runtime::{
    RuntimeApplyOutcome, RuntimeAssistantBlock, RuntimeEvent, RuntimeEventKind, RuntimeFinalStatus,
    RuntimeMessageRole, RuntimeStateDelta, RuntimeToolCall, RuntimeToolStatus,
    RuntimeTranscriptMessage, RuntimeWorktreeNoticeKind,
};
use imp_core::session::SessionEntry;

use crate::views::chat::{DisplayAssistantBlock, DisplayMessage, MessageRole};
use crate::views::tools::DisplayToolCall;

use super::{agent_event_kind, format_error_for_display, verification_status_text, App};

fn terminal_runtime_status_message(status: &RuntimeFinalStatus) -> Option<String> {
    match status {
        RuntimeFinalStatus::Failed { error } => Some(format!(
            "Agent turn failed before producing a response: {error}"
        )),
        RuntimeFinalStatus::Blocked { reason } => Some(format!(
            "Agent turn stopped before producing a response: {reason}"
        )),
        RuntimeFinalStatus::NeedsContext { question } => Some(format!(
            "Agent needs input before it can continue: {question}"
        )),
        RuntimeFinalStatus::Cancelled => {
            Some("Agent turn was cancelled before producing a response.".to_string())
        }
        RuntimeFinalStatus::Done | RuntimeFinalStatus::DoneWithConcerns { .. } => None,
    }
}

impl App {
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        self.trace_first_agent_event(&event);
        let run_id = match &event {
            AgentEvent::AgentStart { timestamp, .. } => {
                let run_id = format!("tui-{timestamp}");
                self.runtime_state.begin_run(run_id.clone());
                self.runtime_event_sequence = 0;
                self.runtime_message_projection_index.retain(|id, _| {
                    self.runtime_state
                        .snapshot_ref()
                        .transcript
                        .iter()
                        .any(|message| message.id == *id)
                });
                run_id
            }
            _ => self
                .runtime_state
                .snapshot_ref()
                .workflow
                .run_id
                .clone()
                .unwrap_or_else(|| "tui-pending".to_string()),
        };
        self.runtime_event_sequence = self.runtime_event_sequence.saturating_add(1);
        let runtime_event = event.to_runtime_event(run_id, self.runtime_event_sequence);
        let outcome = self.runtime_state.apply(&runtime_event);
        let delta = match outcome {
            RuntimeApplyOutcome::Applied(delta) | RuntimeApplyOutcome::Gap { delta, .. } => delta,
            other => {
                self.trace_tui(format!("runtime_event_rejected outcome={other:?}"));
                return;
            }
        };
        self.project_runtime_delta(&runtime_event, &delta);
        self.handle_runtime_ui_effects(&event, &runtime_event);
    }

    fn trace_first_agent_event(&mut self, event: &AgentEvent) {
        if self.first_agent_event_seen {
            return;
        }
        self.first_agent_event_seen = true;
        if let Some(started_at) = self.agent_turn_started_at {
            self.trace_tui(format!(
                "agent_first_event kind={} elapsed_ms={}",
                agent_event_kind(event),
                started_at.elapsed().as_millis()
            ));
        }
    }

    fn project_runtime_delta(&mut self, event: &RuntimeEvent, delta: &RuntimeStateDelta) {
        if delta.workflow_changed
            || delta.phase_changed
            || delta.evidence_changed
            || delta.verification_changed
        {
            for (key, value) in &self.runtime_state.snapshot_ref().status_items {
                self.status_items.insert(key.clone(), value.clone());
            }
            if let Some(model) = &self.runtime_state.snapshot_ref().workflow.model {
                self.model_name = model.clone();
            }
        }
        if delta.verification_changed {
            self.verification_status_items.clear();
            for update in &self.runtime_state.snapshot_ref().verification {
                self.verification_status_items.insert(
                    update.gate.id.clone(),
                    verification_status_text(&update.gate, None, update.closeout_effect),
                );
            }
            for gate in &self.runtime_state.snapshot_ref().verification_gates {
                self.verification_status_items
                    .entry(gate.id.clone())
                    .or_insert_with(|| verification_status_text(gate, None, None));
            }
        }
        if delta.usage_changed {
            self.sync_runtime_usage_projection();
        }
        if delta.transcript_changed || delta.tools_changed {
            self.project_changed_message(delta.changed_message_id.as_deref());
            self.project_changed_tool(delta.changed_tool_call_id.as_deref());
            self.invalidate_chat_render_cache();
        }
        if delta.final_status_changed {
            self.project_terminal_status_message();
        }
        if delta.phase_changed || delta.final_status_changed {
            self.is_streaming = self
                .runtime_state
                .snapshot_ref()
                .active_message_id
                .is_some()
                || matches!(
                    self.runtime_state.snapshot_ref().phase,
                    imp_core::runtime::RuntimePhase::Starting
                        | imp_core::runtime::RuntimePhase::Running
                        | imp_core::runtime::RuntimePhase::WaitingForTool
                        | imp_core::runtime::RuntimePhase::WaitingForApproval
                        | imp_core::runtime::RuntimePhase::Verifying
                );
        }
        if matches!(event.kind, RuntimeEventKind::Error { .. }) {
            self.is_streaming = false;
        }
        self.needs_redraw = true;
    }

    fn project_terminal_status_message(&mut self) {
        let Some(status) = self.runtime_state.snapshot_ref().final_status.as_ref() else {
            return;
        };
        let has_visible_output = self
            .runtime_state
            .snapshot_ref()
            .transcript
            .iter()
            .filter(|message| {
                message.role == RuntimeMessageRole::Assistant && !message.is_streaming
            })
            .any(runtime_message_has_visible_output);
        let Some(message) = (!has_visible_output)
            .then(|| terminal_runtime_status_message(status))
            .flatten()
        else {
            return;
        };
        let message = format_error_for_display(&message);
        if let Some(existing) = self.messages.iter_mut().rev().find(|message| {
            message.role == MessageRole::Assistant
                && (message.is_streaming || message.content.trim().is_empty())
        }) {
            existing.role = MessageRole::Error;
            existing.content = message.clone();
            existing.assistant_blocks = vec![DisplayAssistantBlock::Text(message)];
            existing.is_streaming = false;
        } else {
            self.messages.push(DisplayMessage {
                role: MessageRole::Error,
                content: message.clone(),
                thinking: None,
                tool_calls: Vec::new(),
                assistant_blocks: vec![DisplayAssistantBlock::Text(message)],
                is_streaming: false,
                timestamp: imp_llm::now(),
            });
        }
    }

    fn sync_runtime_usage_projection(&mut self) {
        let usage = &self.runtime_state.snapshot_ref().usage;
        self.accumulated_usage = imp_llm::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        };
        self.accumulated_cost = imp_llm::Cost {
            input: usage.input_cost_micros as f64 / 1_000_000.0,
            output: usage.output_cost_micros as f64 / 1_000_000.0,
            cache_read: usage.cache_read_cost_micros as f64 / 1_000_000.0,
            cache_write: usage.cache_write_cost_micros as f64 / 1_000_000.0,
            total: usage.total_cost_micros as f64 / 1_000_000.0,
        };
        self.current_context_tokens = if self.runtime_state.snapshot_ref().context_usage.used > 0 {
            self.runtime_state.snapshot_ref().context_usage.used
        } else {
            usage.raw_total_tokens
        };
        if self
            .runtime_state
            .snapshot_ref()
            .context_usage
            .display_window
            > 0
        {
            self.context_window = self
                .runtime_state
                .snapshot_ref()
                .context_usage
                .display_window;
        }
    }

    fn project_changed_message(&mut self, message_id: Option<&str>) {
        let Some(message_id) = message_id else {
            return;
        };
        let Some(runtime_message) = self
            .runtime_state
            .snapshot_ref()
            .transcript
            .iter()
            .find(|message| message.id == message_id)
        else {
            return;
        };
        let mut display = project_runtime_message(
            runtime_message,
            &self.runtime_state.snapshot_ref().active_tools,
            &self.runtime_state.snapshot_ref().completed_tools,
            self.tools_expanded
                && self.config.ui.effective_chat_tool_display()
                    == imp_core::config::ChatToolDisplay::Interleaved,
        );
        if runtime_message.role == RuntimeMessageRole::Error {
            display.content = format_error_for_display(&display.content);
            display.assistant_blocks = vec![DisplayAssistantBlock::Text(display.content.clone())];
        }

        if runtime_message.error_replacement {
            if let Some(existing) = self
                .messages
                .iter_mut()
                .rev()
                .find(|message| message.is_streaming)
            {
                *existing = display;
                return;
            }
        }
        if let Some(index) = self
            .runtime_message_projection_index
            .get(message_id)
            .copied()
        {
            if let Some(existing) = self.messages.get_mut(index) {
                *existing = display;
                return;
            }
        }
        if runtime_message.role == RuntimeMessageRole::Assistant {
            if let Some((index, existing)) = self
                .messages
                .iter_mut()
                .enumerate()
                .rev()
                .find(|(_, message)| message.role == MessageRole::Assistant && message.is_streaming)
            {
                *existing = display;
                self.runtime_message_projection_index
                    .insert(message_id.to_string(), index);
                return;
            }
        }
        let index = self.messages.len();
        self.messages.push(display);
        self.runtime_message_projection_index
            .insert(message_id.to_string(), index);
    }

    fn project_changed_tool(&mut self, tool_call_id: Option<&str>) {
        let Some(tool_call_id) = tool_call_id else {
            return;
        };
        let Some(tool) = self
            .runtime_state
            .snapshot_ref()
            .active_tools
            .iter()
            .chain(&self.runtime_state.snapshot_ref().completed_tools)
            .find(|tool| tool.id == tool_call_id)
        else {
            return;
        };
        let projected = project_runtime_tool(
            tool,
            self.tools_expanded
                && self.config.ui.effective_chat_tool_display()
                    == imp_core::config::ChatToolDisplay::Interleaved,
        );
        for message in self.messages.iter_mut().rev() {
            if let Some(existing) = message
                .tool_calls
                .iter_mut()
                .find(|existing| existing.id == tool_call_id)
            {
                *existing = projected;
                return;
            }
        }
    }

    fn handle_runtime_ui_effects(&mut self, event: &AgentEvent, runtime_event: &RuntimeEvent) {
        match event {
            AgentEvent::AgentStart { .. } => self.agent_started_ui_effects(),
            AgentEvent::AgentEnd { status, .. } => self.agent_ended_ui_effects(status),
            AgentEvent::MessageDelta { delta } => self.message_delta_ui_effects(delta),

            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => self.tool_started_ui_effects(tool_call_id, tool_name, args),
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                ..
            } => self.tool_ended_ui_effects(tool_call_id, result),
            AgentEvent::Browser { event } => {
                self.browser_state.apply(event);
                self.status_items
                    .insert("browser".to_string(), self.browser_state.status(event));
                self.invalidate_chat_render_cache();
            }
            AgentEvent::TurnEnd { index, message, .. } => {
                self.persist_assistant_turn(*index, message.clone())
            }

            _ => {}
        }
        self.surface_runtime_transition(runtime_event);
    }

    fn agent_started_ui_effects(&mut self) {
        self.tool_focus = None;
        self.tool_focus_pinned = false;
        self.sidebar_auto_follow = true;
        self.begin_llm_thought_segment();
        self.turn_tracker.clear_counts();
    }

    fn agent_ended_ui_effects(&mut self, _status: &RunFinalStatus) {
        self.streaming_anchor_user_index = None;
        self.completed_turns_in_run = self.runtime_state.snapshot_ref().turns.len().max(1) as u32;
        if !self
            .runtime_state
            .snapshot_ref()
            .transcript
            .iter()
            .any(|message| {
                message.role == RuntimeMessageRole::Assistant
                    && runtime_message_has_visible_output(message)
            })
        {
            if let Some(status) = self.runtime_state.snapshot_ref().final_status.as_ref() {
                if let Some(message) = terminal_runtime_status_message(status) {
                    self.last_agent_error = Some(format_error_for_display(&message));
                }
            }
        }
        let queued: Vec<_> = self.message_queue.drain(..).collect();
        for message in queued {
            self.editor.set_content(message.text());
            self.send_message();
        }
        self.llm_thought_segment_started_at = None;
        self.queue_loop_continuation_if_ready();
        self.maybe_notify_agent_completion();
    }

    fn message_delta_ui_effects(&mut self, delta: &imp_llm::StreamEvent) {
        match delta {
            imp_llm::StreamEvent::TextDelta { text } if !text.trim().is_empty() => {
                if let Some(seconds) = self.finalize_llm_thought_segment() {
                    if let Some(last) = self.latest_streaming_message_mut() {
                        last.push_assistant_thought_duration(seconds);
                    }
                }
            }
            imp_llm::StreamEvent::ToolCall { .. } => {
                let _ = self.finalize_llm_thought_segment();
            }
            _ => {}
        }
    }

    fn tool_started_ui_effects(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) {
        self.turn_tracker
            .record_tool_start(tool_call_id, tool_name, args);
        self.llm_thought_segment_started_at = None;
        if let Some(index) = self.find_tool_call_index(tool_call_id) {
            if !self.tool_focus_pinned {
                self.focus_tool_with_pin(index, false);
            }
            if self.sidebar_auto_follow {
                self.sidebar.detail_scroll = usize::MAX;
            }
        }
        if !self.sidebar.first_tool_seen {
            self.sidebar.first_tool_seen = true;
            let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
            if self.config.ui.effective_chat_tool_display()
                == imp_core::config::ChatToolDisplay::Hidden
                || (self.config.ui.auto_open_sidebar
                    && cols >= self.config.ui.sidebar_auto_open_width)
            {
                self.sidebar.open = true;
            }
        }
    }

    fn tool_ended_ui_effects(&mut self, tool_call_id: &str, result: &imp_llm::ToolResultMessage) {
        self.turn_tracker
            .record_tool_end(tool_call_id, result.is_error);
        self.begin_llm_thought_segment();
        let _ = self.session.append_tool_result_message(result.clone());
    }

    fn persist_assistant_turn(&mut self, index: u32, message: imp_llm::AssistantMessage) {
        if let Some(model_meta) = self.current_model_meta_for_persistence() {
            let _ = self
                .session
                .append_assistant_turn_with_model_meta(&model_meta, index, message);
        } else {
            let _ = self.session.append(SessionEntry::Message {
                id: uuid::Uuid::new_v4().to_string(),
                parent_id: None,
                message: imp_llm::Message::Assistant(message),
            });
        }
    }

    fn surface_runtime_transition(&mut self, event: &RuntimeEvent) {
        match &event.kind {
            RuntimeEventKind::Warning { message } => self.push_warning_msg(message),
            RuntimeEventKind::Error { message } => {
                self.last_agent_error = Some(format_error_for_display(message));
                if let Some(message) =
                    self.messages.iter_mut().rev().find(|message| {
                        message.role == MessageRole::Assistant && message.is_streaming
                    })
                {
                    message.is_streaming = false;
                }
            }
            RuntimeEventKind::PolicyDecision { decision } => {
                if let Some(warning) = &decision.warning {
                    let attached = decision
                        .id
                        .as_deref()
                        .is_some_and(|id| self.add_tool_notice(id, warning));
                    if !attached {
                        self.push_warning_msg(warning);
                    }
                }
            }
            RuntimeEventKind::VerificationCompleted { update }
                if !matches!(
                    update.closeout_effect,
                    Some(imp_core::workflow::VerificationCloseoutEffect::AllowsDone) | None
                ) =>
            {
                self.push_warning_msg(&format!(
                    "Verification failed: {} ({:?})",
                    update.gate.name, update.closeout_effect
                ));
            }
            RuntimeEventKind::ToolCompleted { tool_call } => {
                if let Some(warning) = &tool_call.warning {
                    self.push_warning_msg(warning);
                }
            }
            RuntimeEventKind::WorktreeNotice { notice } => {
                self.push_system_msg(&notice.message);
                let (key, value) = match notice.kind {
                    RuntimeWorktreeNoticeKind::Created => (
                        "worktree",
                        format!(
                            "{} @ {}",
                            notice.worktree.metadata.branch,
                            notice.worktree.metadata.worktree_path.display()
                        ),
                    ),
                    RuntimeWorktreeNoticeKind::DiffCaptured => (
                        "worktree-diff",
                        notice.worktree.metadata.patch_path.display().to_string(),
                    ),
                    RuntimeWorktreeNoticeKind::Closeout => {
                        ("worktree-closeout", notice.message.clone())
                    }
                };
                self.status_items.insert(key.into(), value);
            }
            _ => {}
        }
    }
}

pub(super) fn project_runtime_history(
    snapshot: &imp_core::runtime::RuntimeStateSnapshot,
    expanded: bool,
) -> Vec<DisplayMessage> {
    snapshot
        .transcript
        .iter()
        .map(|message| {
            project_runtime_message(
                message,
                &snapshot.active_tools,
                &snapshot.completed_tools,
                expanded,
            )
        })
        .collect()
}

fn project_runtime_message(
    message: &RuntimeTranscriptMessage,
    active_tools: &[RuntimeToolCall],
    completed_tools: &[RuntimeToolCall],
    expanded: bool,
) -> DisplayMessage {
    let mut display = DisplayMessage {
        role: project_message_role(message.role),
        content: String::new(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: message.is_streaming,
        timestamp: message.timestamp_ms.unwrap_or_default() / 1000,
    };
    for block in &message.blocks {
        match block {
            RuntimeAssistantBlock::VisibleText { text } => display.add_assistant_text_block(text),
            RuntimeAssistantBlock::Thinking { text } => {
                display
                    .thinking
                    .get_or_insert_with(String::new)
                    .push_str(text);
            }
            RuntimeAssistantBlock::ToolCall { tool_call_id } => {
                if let Some(tool) = active_tools
                    .iter()
                    .chain(completed_tools)
                    .find(|tool| tool.id == *tool_call_id)
                {
                    display.push_assistant_tool_call(project_runtime_tool(tool, expanded));
                } else {
                    display
                        .assistant_blocks
                        .push(DisplayAssistantBlock::ToolCall {
                            id: tool_call_id.clone(),
                        });
                }
            }
        }
    }
    display
}

fn project_runtime_tool(tool: &RuntimeToolCall, expanded: bool) -> DisplayToolCall {
    let details = tool
        .details
        .clone()
        .or_else(|| tool.arguments.clone())
        .unwrap_or(serde_json::Value::Null);
    let output = tool.output_preview.clone().filter(|_| {
        !matches!(
            tool.status,
            RuntimeToolStatus::Pending | RuntimeToolStatus::Running
        )
    });
    DisplayToolCall {
        id: tool.id.clone(),
        name: tool.name.clone(),
        args_summary: DisplayToolCall::make_args_summary(&tool.name, &details),
        output,
        details,
        is_error: tool.is_error || tool.status == RuntimeToolStatus::Failed,
        expanded: expanded && (tool.is_error || tool.status == RuntimeToolStatus::Failed),
        notices: tool.warning.clone().into_iter().collect(),
        streaming_lines: tool
            .output_preview
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect(),
        streaming_output: tool.output_preview.clone().unwrap_or_default(),
    }
}

fn project_message_role(role: RuntimeMessageRole) -> MessageRole {
    match role {
        RuntimeMessageRole::User => MessageRole::User,
        RuntimeMessageRole::Assistant => MessageRole::Assistant,
        RuntimeMessageRole::Warning => MessageRole::Warning,
        RuntimeMessageRole::Error => MessageRole::Error,
        RuntimeMessageRole::Compaction => MessageRole::Compaction,
        RuntimeMessageRole::System | RuntimeMessageRole::ToolResult => MessageRole::System,
    }
}

fn runtime_message_has_visible_output(message: &RuntimeTranscriptMessage) -> bool {
    !message.visible_text().trim().is_empty()
        || message
            .blocks
            .iter()
            .any(|block| matches!(block, RuntimeAssistantBlock::ToolCall { .. }))
}
