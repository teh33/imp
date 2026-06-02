use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use imp_core::compaction::COMPACTION_SUMMARY_PREFIX;
use imp_core::session::SessionManager;
use imp_llm::Message;

use crate::views::chat::{DisplayMessage, MessageRole};
use crate::views::session_picker::SessionPickerState;

use super::{
    App, RuntimeSignal, SessionListResult, SessionOpenResult, UiMode, SESSION_LIST_PAGE_SIZE,
    SESSION_LIST_PREFETCH_REMAINING,
};

impl App {
    /// Load messages from the current session branch into display messages.
    pub fn load_session_messages(&mut self) {
        self.messages.clear();
        self.invalidate_chat_render_cache();

        let mut branch_messages: Vec<Message> = self.session.get_active_messages();
        imp_core::session::sanitize_messages(&mut branch_messages);

        for msg in &branch_messages {
            match msg {
                // Attach tool results to their parent tool call display entry
                imp_llm::Message::ToolResult(tr) => {
                    let output_text = tr
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let mut attached = false;
                    for display_msg in self.messages.iter_mut().rev() {
                        for tc in &mut display_msg.tool_calls {
                            if tc.id == tr.tool_call_id {
                                tc.output = Some(output_text.clone());
                                if tc.streaming_output.is_empty() {
                                    tc.streaming_output = output_text.clone();
                                }
                                tc.details = tr.details.clone();
                                tc.is_error = tr.is_error;
                                attached = true;
                                break;
                            }
                        }
                        if attached {
                            break;
                        }
                    }
                    // Only show as standalone if no matching tool call found
                    if !attached {
                        self.messages.push(DisplayMessage::from_message(msg));
                    }
                }
                _ => {
                    let mut display = DisplayMessage::from_message(msg);
                    if matches!(msg, imp_llm::Message::User(_))
                        && display.content.starts_with(COMPACTION_SUMMARY_PREFIX)
                    {
                        display.role = MessageRole::Compaction;
                    }
                    self.messages.push(display);
                }
            }
        }
    }

    pub(super) fn start_session_list_load(&mut self) {
        self.mode = UiMode::SessionPicker(SessionPickerState::loading(Some(&self.cwd)));
        if self.session_list_task.is_some() {
            return;
        }
        let session_dir = imp_core::storage::global_sessions_dir();
        let preferred_cwd = self.cwd.clone();
        let signal_tx = self.runtime_signal_tx.clone();
        self.session_list_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                SessionManager::list_page(&session_dir, 0, SESSION_LIST_PAGE_SIZE, None)
                    .map(|sessions| SessionListResult {
                        sessions,
                        preferred_cwd,
                        offset: 0,
                        limit: SESSION_LIST_PAGE_SIZE,
                    })
                    .map_err(|error| format!("Failed to list sessions: {error}"))
            })
            .await
            {
                Ok(Ok(result)) => RuntimeSignal::SessionListLoaded(result),
                Ok(Err(error)) => RuntimeSignal::SessionListFailed(error),
                Err(error) => {
                    RuntimeSignal::SessionListFailed(format!("Session list task failure: {error}"))
                }
            };
            let _ = signal_tx.send(signal);
        }));
    }

    pub(super) fn start_session_list_page_load(&mut self, offset: usize, query: Option<String>) {
        if self.session_list_task.is_some() {
            return;
        }
        let session_dir = imp_core::storage::global_sessions_dir();
        let preferred_cwd = self.cwd.clone();
        let signal_tx = self.runtime_signal_tx.clone();
        let query_for_task = query.clone();
        self.session_list_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                SessionManager::list_page(
                    &session_dir,
                    offset,
                    SESSION_LIST_PAGE_SIZE,
                    query_for_task.as_deref(),
                )
                .map(|sessions| SessionListResult {
                    sessions,
                    preferred_cwd,
                    offset,
                    limit: SESSION_LIST_PAGE_SIZE,
                })
                .map_err(|error| format!("Failed to list sessions: {error}"))
            })
            .await
            {
                Ok(Ok(result)) => RuntimeSignal::SessionListLoaded(result),
                Ok(Err(error)) => RuntimeSignal::SessionListFailed(error),
                Err(error) => {
                    RuntimeSignal::SessionListFailed(format!("Session list task failure: {error}"))
                }
            };
            let _ = signal_tx.send(signal);
        }));
    }

    pub(super) fn maybe_load_more_sessions(&mut self) {
        let next_offset = match &mut self.mode {
            UiMode::SessionPicker(state)
                if state.should_load_more(SESSION_LIST_PREFETCH_REMAINING) =>
            {
                let next_offset = state.next_offset();
                state.set_loading_more(true);
                next_offset
            }
            _ => return,
        };
        self.start_session_list_page_load(next_offset, None);
    }

    pub(super) fn finish_session_list_load(&mut self, result: SessionListResult) {
        let has_more = result.sessions.len() >= result.limit;
        if result.sessions.is_empty() && result.offset == 0 {
            self.mode = UiMode::Normal;
            self.push_system_msg("No saved sessions found.");
            return;
        }

        let preferred_cwd = result.preferred_cwd;
        if let UiMode::SessionPicker(state) = &mut self.mode {
            if result.offset == 0 {
                state.finish_loading(result.sessions);
                state.has_more = has_more;
                if state.filtered_indices.is_empty() {
                    self.mode = UiMode::Normal;
                    self.push_system_msg("No saved sessions found.");
                }
            } else {
                state.append_sessions(result.sessions, has_more);
            }
        } else {
            let mut state = SessionPickerState::new(result.sessions, Some(&preferred_cwd));
            state.has_more = has_more;
            self.mode = UiMode::SessionPicker(state);
        }
    }

    pub(super) fn fail_session_list_load(&mut self, error: String) {
        if let UiMode::SessionPicker(state) = &mut self.mode {
            state.fail_loading();
            self.mode = UiMode::Normal;
        }
        self.push_error_msg(&error);
    }

    pub(super) fn start_session_open(&mut self, path: PathBuf) {
        if self.session_open_task.is_some() {
            return;
        }
        self.mode = UiMode::Normal;
        self.push_system_msg("Resuming session…");
        let signal_tx = self.runtime_signal_tx.clone();
        self.session_open_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                let session = SessionManager::open(&path)
                    .map_err(|error| format!("Failed to open session: {error}"))?;
                let summary = session.summary().map(str::to_string);
                Ok(SessionOpenResult { session, summary })
            })
            .await
            {
                Ok(Ok(result)) => RuntimeSignal::SessionOpened(result),
                Ok(Err(error)) => RuntimeSignal::SessionOpenFailed(error),
                Err(error) => {
                    RuntimeSignal::SessionOpenFailed(format!("Session open task failure: {error}"))
                }
            };
            let _ = signal_tx.send(signal);
        }));
    }

    pub(super) fn finish_session_open(&mut self, result: SessionOpenResult) {
        self.session = result.session;
        self.load_session_messages();
        if let Some(summary) = result.summary {
            self.push_system_msg(&format!("Session resumed — {summary}"));
        } else {
            self.push_system_msg("Session resumed.");
        }
    }

    pub(super) fn handle_session_picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = UiMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let UiMode::SessionPicker(ref mut state) = self.mode {
                    state.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let UiMode::SessionPicker(ref mut state) = self.mode {
                    state.move_down();
                }
                self.maybe_load_more_sessions();
            }
            KeyCode::Backspace => {
                if let UiMode::SessionPicker(ref mut state) = self.mode {
                    state.pop_filter();
                }
            }
            KeyCode::Char(c) if !c.is_control() => {
                if let UiMode::SessionPicker(ref mut state) = self.mode {
                    state.push_filter(c);
                }
            }
            KeyCode::Enter => {
                let selected_path = if let UiMode::SessionPicker(ref state) = self.mode {
                    state.selected_session().map(|s| s.path.clone())
                } else {
                    None
                };
                if let Some(path) = selected_path {
                    self.start_session_open(path);
                }
            }
            _ => {}
        }
    }
}
