use std::path::PathBuf;

use imp_core::session::{SessionEntry, SessionManager};
use imp_llm::{truncate_chars_with_suffix, ContentBlock, Message, UserMessage};

use crate::views::chat::{DisplayMessage, MessageRole};
use crate::views::tools::DisplayToolCall;

use super::{single_line_preview, App, RuntimeSignal};

impl App {
    /// Find a tool call's flat index by ID across all display messages.
    pub(super) fn find_tool_call_index(&self, id: &str) -> Option<usize> {
        let mut index = 0;
        for msg in &self.messages {
            for tc in &msg.tool_calls {
                if tc.id == id {
                    return Some(index);
                }
                index += 1;
            }
        }
        None
    }

    pub(super) fn enqueue_visible_agent_turn(&mut self, text: String) {
        self.enqueue_visible_agent_turn_with_prompt(text.clone(), text);
    }

    pub(super) fn enqueue_visible_agent_turn_with_prompt(
        &mut self,
        visible_text: String,
        agent_prompt: String,
    ) {
        let user_message_index = self.messages.len();
        let timestamp = imp_llm::now();
        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: visible_text.clone(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: false,
            timestamp,
        });
        self.messages.push(DisplayMessage {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: true,
            timestamp: imp_llm::now(),
        });
        self.invalidate_chat_render_cache();
        let entry_id = uuid::Uuid::new_v4().to_string();
        let persist_session = self.session.clone();
        let agent_session = self.session.snapshot_with_pending_user_message(
            entry_id.clone(),
            timestamp,
            visible_text.clone(),
        );
        self.start_user_message_persist(persist_session, entry_id, visible_text.clone(), timestamp);
        self.session = agent_session;

        self.is_streaming = true;
        self.streaming_anchor_user_index = Some(user_message_index);
        self.completed_turns_in_run = 0;
        self.last_agent_error = None;
        self.suppress_completion_notification = false;
        self.auto_scroll = true;
        self.scroll_offset = 0;
        self.tool_focus = None;
        self.tool_focus_pinned = false;
        self.sidebar_auto_follow = true;
        self.set_pending_agent_turn(agent_prompt, Some(visible_text), None);
    }

    pub(super) fn set_pending_agent_turn(
        &mut self,
        agent_prompt: String,
        visible_text: Option<String>,
        cwd: Option<PathBuf>,
    ) {
        self.pending_agent_prompt = Some(agent_prompt);
        self.pending_agent_visible_text = visible_text;
        self.pending_agent_cwd = cwd;
        self.needs_redraw = true;
    }

    pub(super) fn clear_pending_agent_turn(&mut self) {
        self.pending_agent_prompt = None;
        self.pending_agent_visible_text = None;
        self.pending_agent_cwd = None;
    }

    pub(super) fn push_system_msg(&mut self, content: &str) {
        self.push_message(MessageRole::System, content);
    }

    pub(super) fn push_warning_msg(&mut self, content: &str) {
        self.push_message(MessageRole::Warning, content);
    }

    pub(super) fn push_error_msg(&mut self, content: &str) {
        self.push_message(MessageRole::Error, content);
    }

    pub(super) fn replace_empty_streaming_with_error(&mut self, content: &str) -> bool {
        let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.is_streaming)
        else {
            return false;
        };

        let has_visible_output = !message.content.trim().is_empty()
            || message
                .thinking
                .as_deref()
                .is_some_and(|thinking| !thinking.trim().is_empty())
            || !message.tool_calls.is_empty()
            || !message.assistant_blocks.is_empty();
        if has_visible_output {
            message.is_streaming = false;
            self.invalidate_chat_render_cache();
            return false;
        }

        message.role = MessageRole::Error;
        message.content = content.to_string();
        message.thinking = None;
        message.tool_calls.clear();
        message.assistant_blocks.clear();
        message.is_streaming = false;
        self.invalidate_chat_render_cache();
        true
    }

    pub(super) fn push_message(&mut self, role: MessageRole, content: &str) {
        self.messages.push(DisplayMessage {
            role,
            content: content.to_string(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: false,
            timestamp: imp_llm::now(),
        });
        self.invalidate_chat_render_cache();
    }

    pub(super) fn latest_streaming_message_mut(&mut self) -> Option<&mut DisplayMessage> {
        self.messages.iter_mut().rev().find(|msg| msg.is_streaming)
    }

    pub(super) fn find_tool_call_mut(
        &mut self,
        tool_call_id: &str,
    ) -> Option<&mut DisplayToolCall> {
        for msg in self.messages.iter_mut().rev() {
            if let Some(tc) = msg.tool_calls.iter_mut().find(|tc| tc.id == tool_call_id) {
                return Some(tc);
            }
        }
        None
    }

    pub(super) fn queued_message_preview(&self, terminal_width: u16) -> Option<String> {
        let text = self.message_queue.first()?.text();
        let max_chars = (terminal_width as usize / 2).max(8);
        Some(truncate_chars_with_suffix(
            &single_line_preview(text),
            max_chars,
            "…",
        ))
    }

    pub(super) fn start_user_message_persist(
        &mut self,
        session: SessionManager,
        entry_id: String,
        prompt: String,
        timestamp: u64,
    ) {
        if self.user_message_persist_task.is_some() {
            self.trace_tui("user_message_persist skipped_existing_task");
            return;
        }

        let mut session = session;
        let signal_tx = self.runtime_signal_tx.clone();
        self.user_message_persist_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                let message = Message::User(UserMessage {
                    content: vec![ContentBlock::Text { text: prompt }],
                    timestamp,
                });
                let entry = SessionEntry::Message {
                    id: entry_id.clone(),
                    parent_id: session.leaf_id().map(str::to_string),
                    message,
                };
                let entry_id = entry.id().unwrap_or_default().to_string();
                session
                    .append(entry)
                    .map(|_| (entry_id, session.path().is_some().then_some(session)))
                    .map_err(|error| format!("Failed to persist user message: {error}"))
            })
            .await
            {
                Ok(Ok((entry_id, persisted_session))) => RuntimeSignal::UserMessagePersisted {
                    entry_id,
                    persisted_session,
                },
                Ok(Err(error)) => RuntimeSignal::UserMessagePersistFailed(error),
                Err(error) => RuntimeSignal::UserMessagePersistFailed(format!(
                    "User message persist task failure: {error}"
                )),
            };
            let _ = signal_tx.send(signal);
        }));
    }

    pub(super) fn finish_user_message_persist(
        &mut self,
        entry_id: String,
        persisted_session: Option<SessionManager>,
    ) {
        self.user_message_persist_task = None;
        if let Some(session) = persisted_session {
            self.session = session;
        } else if self.session.path().is_none() {
            self.session.set_leaf_id_for_in_memory(entry_id);
        }
    }
}
