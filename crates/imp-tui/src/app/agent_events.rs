use imp_core::agent::{AgentEvent, RunFinalStatus};
use imp_core::session::SessionEntry;
use imp_core::workflow::VerificationCloseoutEffect;
use imp_llm::StreamEvent;

use crate::views::chat::{DisplayMessage, MessageRole};
use crate::views::tools::DisplayToolCall;

use super::{
    agent_event_kind, extension_policy_warning, format_error_for_display, provenance_warning,
    trust_policy_warning, verification_gate_label, verification_status_text, App,
};

fn terminal_agent_status_message(status: &RunFinalStatus) -> Option<String> {
    match status {
        RunFinalStatus::Failed { message } => Some(format!(
            "Agent turn failed before producing a response: {message}"
        )),
        RunFinalStatus::Blocked { message, .. } => Some(format!(
            "Agent turn stopped before producing a response: {message}"
        )),
        RunFinalStatus::NeedsUserInput { question } => Some(format!(
            "Agent needs input before it can continue: {question}"
        )),
        RunFinalStatus::Cancelled => {
            Some("Agent turn was cancelled before producing a response.".to_string())
        }
        RunFinalStatus::Done { .. } | RunFinalStatus::DoneWithConcerns { .. } => None,
    }
}

impl App {
    // ── Agent event handling ────────────────────────────────────

    fn apply_runtime_snapshot_projection(&mut self) {
        self.runtime_snapshot = self.runtime_state.snapshot();

        for (key, value) in &self.runtime_snapshot.status_items {
            self.status_items.insert(key.clone(), value.clone());
        }

        self.verification_status_items.clear();
        for gate in &self.runtime_snapshot.verification_gates {
            let status = format!("{:?}", gate.status).to_lowercase();
            self.verification_status_items.insert(
                gate.id.clone(),
                verification_status_text(gate, Some(&status), None),
            );
        }
    }

    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        self.runtime_event_sequence += 1;
        let runtime_event = event.to_runtime_event("tui", self.runtime_event_sequence);
        self.runtime_state.apply(&runtime_event);
        self.apply_runtime_snapshot_projection();
        self.handle_agent_event_legacy(event)
    }

    fn handle_agent_event_legacy(&mut self, event: AgentEvent) {
        if !self.first_agent_event_seen {
            self.first_agent_event_seen = true;
            if let Some(started_at) = self.agent_turn_started_at {
                self.trace_tui(format!(
                    "agent_first_event kind={} elapsed_ms={}",
                    agent_event_kind(&event),
                    started_at.elapsed().as_millis()
                ));
            }
        }
        match event {
            AgentEvent::AgentStart { model, .. } => {
                self.model_name = model;
                self.is_streaming = true;
                self.tool_focus = None;
                self.tool_focus_pinned = false;
                self.sidebar_auto_follow = true;
                self.invalidate_chat_render_cache();
                self.begin_llm_thought_segment();
                self.turn_tracker.clear_counts();
            }
            AgentEvent::TurnStart { .. } => {
                self.is_streaming = true;
                if !self.messages.iter().any(|message| message.is_streaming) {
                    self.messages.push(DisplayMessage {
                        role: MessageRole::Assistant,
                        content: String::new(),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: true,
                        timestamp: imp_llm::now(),
                    });
                    self.streaming_anchor_user_index = self
                        .messages
                        .iter()
                        .rposition(|message| message.role == MessageRole::User);
                    self.invalidate_chat_render_cache();
                }
            }
            AgentEvent::AgentEnd { cost, status, .. } => {
                let had_visible_turn_output = self.completed_turns_in_run > 0
                    || self.latest_streaming_message_mut().is_some_and(|message| {
                        !message.content.trim().is_empty() || !message.tool_calls.is_empty()
                    });
                self.completed_turns_in_run = self.completed_turns_in_run.max(1);
                self.accumulated_cost.total += cost.total;
                self.accumulated_cost.input += cost.input;
                self.accumulated_cost.output += cost.output;
                self.is_streaming = false;
                self.streaming_anchor_user_index = None;

                if !had_visible_turn_output {
                    if let Some(message) = terminal_agent_status_message(&status) {
                        let display_error = format_error_for_display(&message);
                        if self.last_agent_error.as_deref() != Some(display_error.as_str()) {
                            self.last_agent_error = Some(display_error.clone());
                            self.replace_empty_streaming_with_error(&display_error);
                        }
                    }
                }

                // Mark last streaming message as done
                if let Some(last) = self.latest_streaming_message_mut() {
                    last.is_streaming = false;
                }
                self.invalidate_chat_render_cache();

                // Process queued messages. Follow-ups become visible user turns
                // and start the next agent run; steering messages that were still
                // queued at turn end are also surfaced and sent as the next prompt.
                let queued: Vec<_> = self.message_queue.drain(..).collect();
                for message in queued {
                    let text = message.text().to_string();
                    self.editor.set_content(&text);
                    self.send_message();
                }
                self.llm_thought_segment_started_at = None;
                self.queue_loop_continuation_if_ready();
                self.maybe_notify_agent_completion();
            }
            AgentEvent::MessageDelta { delta } => {
                // Keep the current default compact: the main transcript shows
                // where the tool ran, and the sidebar inspector owns details.
                let tools_expanded = self.tools_expanded
                    && self.config.ui.effective_chat_tool_display()
                        == imp_core::config::ChatToolDisplay::Interleaved;
                let thought_duration = match &delta {
                    StreamEvent::TextDelta { text } if !text.trim().is_empty() => {
                        self.finalize_llm_thought_segment()
                    }
                    StreamEvent::ToolCall { .. } => self.finalize_llm_thought_segment(),
                    _ => None,
                };
                if let Some(last) = self.latest_streaming_message_mut() {
                    match delta {
                        StreamEvent::TextDelta { text } => {
                            if let Some(seconds) = thought_duration {
                                last.push_assistant_thought_duration(seconds);
                            }
                            last.push_assistant_text_delta(&text);
                        }
                        StreamEvent::ThinkingDelta { text } => match &mut last.thinking {
                            Some(t) => t.push_str(&text),
                            None => last.thinking = Some(text),
                        },
                        StreamEvent::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            if let Some(seconds) = thought_duration {
                                last.push_assistant_thought_duration(seconds);
                            }
                            last.push_assistant_tool_call(DisplayToolCall {
                                id,
                                args_summary: DisplayToolCall::make_args_summary(&name, &arguments),
                                name,
                                output: None,
                                details: arguments,
                                is_error: false,
                                expanded: tools_expanded,
                                streaming_lines: Vec::new(),
                                streaming_output: String::new(),
                            });
                        }
                        _ => {}
                    }
                }
                self.invalidate_chat_render_cache();
                self.needs_redraw = true;
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.turn_tracker
                    .record_tool_start(&tool_call_id, &tool_name, &args);
                self.llm_thought_segment_started_at = None;
                // Find the matching tool call and update it
                if let Some(tc) = self.find_tool_call_mut(&tool_call_id) {
                    tc.args_summary = DisplayToolCall::make_args_summary(&tool_name, &args);
                    tc.details = args;
                }
                self.invalidate_chat_render_cache();
                // Sidebar: follow the new tool only until the user pins an older selection.
                if let Some(idx) = self.find_tool_call_index(&tool_call_id) {
                    if !self.tool_focus_pinned {
                        self.focus_tool_with_pin(idx, false);
                    }
                    if self.sidebar_auto_follow
                        && matches!(
                            self.config.ui.sidebar_style,
                            imp_core::config::SidebarStyle::Stream
                                | imp_core::config::SidebarStyle::Inspector
                        )
                    {
                        self.sidebar.detail_scroll = usize::MAX;
                    }
                }
                // Auto-open on first tool if terminal is wide enough, or whenever
                // chat tool calls are hidden and the sidebar is their only surface.
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
            AgentEvent::ToolOutputDelta { tool_call_id, text } => {
                let streaming_lines_limit = self.config.ui.streaming_lines;
                // Feed streaming output into the tool call's rolling buffer
                if let Some(tc) = self.find_tool_call_mut(&tool_call_id) {
                    // Append text to the full live transcript.
                    if !tc.streaming_output.is_empty() {
                        tc.streaming_output.push('\n');
                    }
                    tc.streaming_output.push_str(&text);
                    // Append text and keep configured rolling tail for chat.
                    for line in text.lines() {
                        tc.streaming_lines.push(line.to_string());
                    }
                    if tc.streaming_lines.len() > streaming_lines_limit {
                        let excess = tc.streaming_lines.len() - streaming_lines_limit;
                        tc.streaming_lines.drain(..excess);
                    }
                }
                self.invalidate_chat_render_cache();
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                provenance,
            } => {
                if let Some(provenance) = provenance.as_ref() {
                    if let Some(message) = provenance_warning(provenance) {
                        self.push_warning_msg(&message);
                    }
                }
                let is_error = result.is_error;
                self.turn_tracker.record_tool_end(&tool_call_id, is_error);
                self.begin_llm_thought_segment();
                // Build display text from result content
                let output_text = result
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let inline_output_enabled = self.config.ui.effective_chat_tool_display()
                    == imp_core::config::ChatToolDisplay::Interleaved;
                // Attach result to the matching display tool call
                if let Some(tc) = self.find_tool_call_mut(&tool_call_id) {
                    tc.output = Some(output_text.clone());
                    if tc.streaming_output.is_empty() {
                        tc.streaming_output = output_text.clone();
                    }
                    tc.details = result.details.clone();
                    tc.is_error = is_error;
                    // Auto-expand failed tool calls so the error is immediately visible
                    // when inline tool output is enabled. In the default inspector flow,
                    // the selected sidebar owns full error details instead.
                    if is_error {
                        tc.expanded = inline_output_enabled;
                    }
                }

                self.invalidate_chat_render_cache();

                // Persist tool result to session so resume has full conversation
                let _ = self.session.append_tool_result_message(result);
            }
            AgentEvent::Browser { event } => {
                self.status_items.insert(
                    "browser".to_string(),
                    format!(
                        "{:?} · {}",
                        event.kind,
                        event.domain.as_deref().unwrap_or("Lightpanda")
                    ),
                );
                self.invalidate_chat_render_cache();
            }
            AgentEvent::ContextUsageUpdated {
                used,
                display_window,
                ..
            } => {
                self.current_context_tokens = used;
                self.context_window = display_window;
                self.invalidate_chat_render_cache();
            }
            AgentEvent::Warning { message } => {
                self.push_warning_msg(&message);
            }
            AgentEvent::RecoveryCheckpoint { .. } => {}
            AgentEvent::WorktreeCreated { metadata } => {
                self.status_items.insert(
                    "worktree".to_string(),
                    format!("{} @ {}", metadata.branch, metadata.worktree_path.display()),
                );
                self.push_system_msg(&format!(
                    "Worktree-auto active: editing {} on branch {} (original checkout: {}).",
                    metadata.worktree_path.display(),
                    metadata.branch,
                    metadata.main_worktree.display()
                ));
                self.invalidate_chat_render_cache();
            }
            AgentEvent::WorktreeDiffCaptured { metadata } => {
                self.status_items.insert(
                    "worktree-diff".to_string(),
                    metadata.patch_path.display().to_string(),
                );
                self.push_system_msg(&format!(
                    "Worktree diff captured: {}. Closeout choices: keep worktree, apply patch, or discard worktree.",
                    metadata.patch_path.display()
                ));
                self.invalidate_chat_render_cache();
            }
            AgentEvent::WorktreeCloseout { result } => {
                self.status_items.insert(
                    "worktree-closeout".to_string(),
                    format!("{:?}: {}", result.action, result.message),
                );
                self.push_system_msg(&format!("Worktree closeout: {}", result.message));
                self.invalidate_chat_render_cache();
            }
            AgentEvent::EvidenceWritten { path } => {
                self.status_items
                    .insert("evidence".to_string(), path.display().to_string());
                self.invalidate_chat_render_cache();
            }
            AgentEvent::VerificationStarted { gate } => {
                self.verification_status_items.insert(
                    gate.id.clone(),
                    verification_status_text(&gate, Some("running"), None),
                );
            }
            AgentEvent::VerificationCompleted {
                gate,
                closeout_effect,
            } => {
                let status = format!("{:?}", gate.status).to_lowercase();
                self.verification_status_items.insert(
                    gate.id.clone(),
                    verification_status_text(&gate, Some(&status), Some(closeout_effect)),
                );
                if !matches!(closeout_effect, VerificationCloseoutEffect::AllowsDone) {
                    self.push_warning_msg(&format!(
                        "Verification {}: {} ({:?})",
                        status,
                        verification_gate_label(&gate),
                        closeout_effect
                    ));
                    self.invalidate_chat_render_cache();
                }
            }
            AgentEvent::PolicyChecked { record } => {
                if let Some(message) = trust_policy_warning(&record) {
                    self.push_warning_msg(&message);
                }
                if let Some(message) = extension_policy_warning(&record) {
                    self.push_warning_msg(&message);
                }
            }
            AgentEvent::Timing { timing } => {
                self.status_items.insert("timing".to_string(), {
                    let label = timing
                        .label
                        .as_deref()
                        .map(|label| format!(" {label}"))
                        .unwrap_or_default();
                    let duration = timing
                        .duration_ms
                        .map(|ms| format!(" duration={ms}ms"))
                        .unwrap_or_default();
                    let elapsed = timing
                        .since_llm_request_start_ms
                        .map(|ms| format!(" llm={ms}ms"))
                        .unwrap_or_else(|| format!(" turn={}ms", timing.since_turn_start_ms));
                    format!("{}{}{}{}", timing.stage.as_str(), label, elapsed, duration)
                });
            }
            AgentEvent::TurnEnd {
                index,
                message,
                workflow_review: _,
            } => {
                self.completed_turns_in_run += 1;
                // Use the provider's latest active context accounting as the
                // canonical display baseline. OpenAI reports cached tokens as a
                // subset of input tokens, so adding cache_read_tokens would
                // double-count them and diverge from the provider window.
                if let Some(ref usage) = message.usage {
                    self.current_context_tokens = usage.raw_total_tokens();
                    self.accumulated_usage.add(usage);
                }

                // Persist assistant message to session, plus canonical usage when possible.
                if let Some(model_meta) = self.current_model_meta_for_persistence() {
                    let _ = self.session.append_assistant_turn_with_model_meta(
                        &model_meta,
                        index,
                        message,
                    );
                } else {
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let _ = self.session.append(SessionEntry::Message {
                        id: msg_id,
                        parent_id: None,
                        message: imp_llm::Message::Assistant(message),
                    });
                }
            }
            AgentEvent::Error { error } => {
                self.completed_turns_in_run = 0;
                // Stop streaming — errors can be terminal (no AgentEnd follows)
                self.is_streaming = false;
                self.streaming_anchor_user_index = None;

                // Parse the error for a cleaner display
                let display_error = format_error_for_display(&error);
                if self.last_agent_error.as_deref() == Some(display_error.as_str()) {
                    if !self.replace_empty_streaming_with_error(&display_error) {
                        self.invalidate_chat_render_cache();
                    }
                    return;
                }
                self.last_agent_error = Some(display_error.clone());

                if !self.replace_empty_streaming_with_error(&display_error) {
                    self.messages.push(DisplayMessage {
                        role: MessageRole::Error,
                        content: display_error,
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                }
                self.invalidate_chat_render_cache();
            }
            _ => {}
        }
    }
}
