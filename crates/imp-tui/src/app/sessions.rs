use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use imp_core::runtime::{RuntimeEvent, RuntimeEventKind, RuntimeSessionProjection};
use imp_core::session::SessionManager;
use imp_llm::Message;

use crate::views::session_picker::SessionPickerState;

use super::{
    agent_events::project_runtime_history, App, RuntimeSignal, SessionListResult,
    SessionOpenResult, UiMode, SESSION_LIST_PAGE_SIZE, SESSION_LIST_PREFETCH_REMAINING,
};

impl App {
    /// Load the current durable session branch into authoritative runtime state,
    /// then project that state into display messages.
    pub fn load_session_messages(&mut self) {
        let mut branch_messages: Vec<Message> = self.session.get_active_messages();
        imp_core::session::sanitize_messages(&mut branch_messages);
        let projection = RuntimeSessionProjection::from_messages(&branch_messages);
        let run_id = self
            .runtime_state
            .snapshot_ref()
            .workflow
            .run_id
            .clone()
            .unwrap_or_else(|| "tui-pending".to_string());
        self.runtime_state.reset(run_id.clone());
        self.runtime_event_sequence = 1;
        let hydration = RuntimeEvent {
            run_id,
            sequence: self.runtime_event_sequence,
            kind: RuntimeEventKind::SessionHydrated {
                transcript: projection.transcript,
                completed_tools: projection.completed_tools,
            },
            ..RuntimeEvent::default()
        };
        let _ = self.runtime_state.apply(&hydration);
        let expanded = self.tools_expanded
            && self.config.ui.effective_chat_tool_display()
                == imp_core::config::ChatToolDisplay::Interleaved;
        self.messages = project_runtime_history(self.runtime_state.snapshot_ref(), expanded);
        self.runtime_message_projection_index = self
            .runtime_state
            .snapshot_ref()
            .transcript
            .iter()
            .enumerate()
            .map(|(index, message)| (message.id.clone(), index))
            .collect();
        self.invalidate_chat_render_cache();
    }

    pub(super) fn start_session_list_load(&mut self) {
        self.mode = UiMode::SessionPicker(SessionPickerState::loading(Some(&self.cwd)));
        if self.session_list_task.is_some() {
            return;
        }
        let session_dirs = imp_core::storage::session_dirs_for_read();
        let preferred_cwd = self.cwd.clone();
        let signal_tx = self.runtime_signal_tx.clone();
        self.session_list_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                SessionManager::list_resumable_page_from_dirs(
                    &session_dirs,
                    0,
                    SESSION_LIST_PAGE_SIZE,
                    None,
                )
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
        let session_dirs = imp_core::storage::session_dirs_for_read();
        let preferred_cwd = self.cwd.clone();
        let signal_tx = self.runtime_signal_tx.clone();
        let query_for_task = query.clone();
        self.session_list_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                SessionManager::list_resumable_page_from_dirs(
                    &session_dirs,
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
        self.session_list_task = None;
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
        self.session_list_task = None;
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
