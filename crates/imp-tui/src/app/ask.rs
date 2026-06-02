use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use imp_core::agent::AgentCommand;
use imp_core::ui::SelectionAnswer;
use imp_llm::auth::AuthStore;

use crate::keybindings::{self, Action};
use crate::views::ask_bar::AskState;
use crate::views::chat::{DisplayMessage, MessageRole};

use super::{parse_secret_field_names, App, AskReply, SecretsFlowState};

impl App {
    pub(super) fn handle_ui_request(&mut self, req: crate::tui_interface::UiRequest) {
        use crate::tui_interface::UiRequest;
        use crate::views::ask_bar::{AskOption, AskState};

        match req {
            UiRequest::Select {
                title,
                context,
                options,
                reply,
            } => {
                let ask_options: Vec<AskOption> = options
                    .into_iter()
                    .map(|o| AskOption {
                        label: o.label,
                        description: o.description,
                        checked: false,
                    })
                    .collect();
                self.begin_ask(
                    AskState::with_placeholder(
                        title,
                        context,
                        ask_options,
                        false,
                        "type to filter or answer freely…".into(),
                    ),
                    AskReply::Select(reply),
                );
            }
            UiRequest::SelectOrInput {
                title,
                context,
                options,
                placeholder,
                reply,
            } => {
                let ask_options: Vec<AskOption> = options
                    .into_iter()
                    .map(|o| AskOption {
                        label: o.label,
                        description: o.description,
                        checked: false,
                    })
                    .collect();
                self.begin_ask(
                    AskState::with_placeholder(title, context, ask_options, false, placeholder),
                    AskReply::SelectOrInput(reply),
                );
            }
            UiRequest::MultiSelect {
                title,
                context,
                options,
                reply,
            } => {
                let ask_options: Vec<AskOption> = options
                    .into_iter()
                    .map(|o| AskOption {
                        label: o.label,
                        description: o.description,
                        checked: false,
                    })
                    .collect();
                self.begin_ask(
                    AskState::with_placeholder(
                        title,
                        context,
                        ask_options,
                        true,
                        "type to answer freely…".into(),
                    ),
                    AskReply::MultiSelect(reply),
                );
            }
            UiRequest::Input {
                title,
                context,
                placeholder,
                reply,
            } => {
                self.begin_ask(
                    AskState::with_placeholder(title, context, vec![], false, placeholder),
                    AskReply::Input(reply),
                );
            }
            UiRequest::Confirm {
                title,
                message,
                reply,
            } => {
                let options = vec![
                    AskOption {
                        label: "Yes".into(),
                        description: None,
                        checked: false,
                    },
                    AskOption {
                        label: "No".into(),
                        description: None,
                        checked: false,
                    },
                ];
                let (bool_tx, bool_rx) = tokio::sync::oneshot::channel();
                self.begin_ask(
                    AskState::with_placeholder(title, message, options, false, String::new()),
                    AskReply::Select(bool_tx),
                );
                let confirm_reply = reply;
                tokio::spawn(async move {
                    let result = bool_rx.await.ok().flatten();
                    let _ = confirm_reply.send(result.map(|idx| idx == 0));
                });
            }
            UiRequest::Notify { message, level } => match level {
                imp_core::ui::NotifyLevel::Error => self.push_error_msg(&message),
                imp_core::ui::NotifyLevel::Warning => self.push_warning_msg(&message),
                imp_core::ui::NotifyLevel::Info => self.push_system_msg(&message),
            },
            UiRequest::SetStatus { key, text } => {
                if let Some(t) = text {
                    self.status_items.insert(key, t);
                } else {
                    self.status_items.remove(&key);
                }
            }
            UiRequest::SetWidget { key, content } => {
                if let Some(content) = content {
                    self.widgets.insert(key, content);
                } else {
                    self.widgets.remove(&key);
                }
            }
            UiRequest::Custom { reply, .. } => {
                let _ = reply.send(None);
            }
        }
    }

    pub(super) fn begin_ask(&mut self, mut state: AskState, reply: AskReply) {
        if self.ask_state.is_none() {
            self.ask_editor_backup = Some(self.editor.clone());
            self.editor.clear();
        }
        state.sync_from_editor(self.editor.content(), self.editor.cursor);
        self.ask_state = Some(state);
        self.ask_reply = Some(reply);
    }

    pub(super) fn sync_ask_from_editor(&mut self) {
        if let Some(state) = self.ask_state.as_mut() {
            state.sync_from_editor(self.editor.content(), self.editor.cursor);
        }
    }

    pub(super) fn restore_editor_after_ask(&mut self) {
        if let Some(saved) = self.ask_editor_backup.take() {
            self.editor = saved;
        } else {
            self.editor.clear();
        }
    }

    pub(super) fn handle_ask_key(&mut self, key: KeyEvent) {
        if self.is_paste_shortcut(key) {
            self.paste_from_clipboard();
            return;
        }

        let Some(state) = self.ask_state.as_ref() else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                self.cancel_ask();
            }
            KeyCode::Enter => {
                self.sync_ask_from_editor();
                self.finish_ask();
            }
            KeyCode::Tab => {
                let replacement = if !state.options.is_empty() && !state.input_active {
                    let cursor = state.cursor.min(state.options.len().saturating_sub(1));
                    state.options.get(cursor).map(|opt| opt.label.clone())
                } else {
                    None
                };
                if let Some(text) = replacement {
                    self.editor.set_content(&text);
                    self.editor.move_end();
                    self.sync_ask_from_editor();
                }
            }
            KeyCode::Char(' ') if !state.input_active => {
                if let Some(state) = self.ask_state.as_mut() {
                    state.toggle_current();
                }
            }
            KeyCode::Char(c) if !state.input_active && c.is_ascii_digit() => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                let quick_selected = if let Some(state) = self.ask_state.as_mut() {
                    state.quick_select(n)
                } else {
                    false
                };
                if quick_selected {
                    self.finish_ask();
                }
            }
            KeyCode::Up => {
                if let Some(state) = self.ask_state.as_mut() {
                    if state.input_active {
                        if !self.editor.move_up() {
                            self.editor.move_home();
                        }
                        self.sync_ask_from_editor();
                    } else {
                        state.cursor_up();
                    }
                }
            }
            KeyCode::Down => {
                if let Some(state) = self.ask_state.as_mut() {
                    if state.input_active {
                        if !self.editor.move_down() {
                            self.editor.move_end();
                        }
                        self.sync_ask_from_editor();
                    } else {
                        state.cursor_down();
                    }
                }
            }
            _ => {
                if let Some(action) = keybindings::resolve_normal(key) {
                    match action {
                        Action::InsertChar(c) => self.editor.insert_char(c),
                        Action::Backspace => self.editor.delete_back(),
                        Action::Delete => self.editor.delete_forward(),
                        Action::CursorLeft => self.editor.move_left(),
                        Action::CursorRight => self.editor.move_right(),
                        Action::CursorHome => self.editor.move_home(),
                        Action::CursorEnd => self.editor.move_end(),
                        Action::WordLeft => self.editor.move_word_left(),
                        Action::WordRight => self.editor.move_word_right(),
                        Action::DeleteWordBack => self.editor.delete_word_back(),
                        Action::DeleteToStart => self.editor.delete_to_start(),
                        Action::DeleteToEnd => self.editor.delete_to_end(),
                        Action::NewLine => self.editor.insert_newline(),
                        _ => {}
                    }
                    self.sync_ask_from_editor();
                }
            }
        }
    }

    pub(super) fn finish_ask(&mut self) {
        use crate::views::ask_bar::AskResult;

        self.sync_ask_from_editor();
        let state = self.ask_state.take();
        let reply = self.ask_reply.take();

        let Some(state) = state else { return };
        let result = state.confirm();
        self.restore_editor_after_ask();

        // Show Q&A in chat as user-style messages so they stay visually distinct
        // (System messages render muted/grey which makes them look faded.)
        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: state.question.clone(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: false,
            timestamp: imp_llm::now(),
        });

        match (&result, reply) {
            (AskResult::Text(text), Some(AskReply::Input(tx))) => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::User,
                    content: text.clone(),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.invalidate_chat_render_cache();
                let _ = tx.send(Some(text.clone()));
                self.advance_secrets_flow(Some(text.clone()));
            }
            (AskResult::Selected(indices), Some(AskReply::Select(tx))) => {
                let labels: Vec<String> = indices
                    .iter()
                    .filter_map(|&i| state.options.get(i).map(|o| o.label.clone()))
                    .collect();
                self.messages.push(DisplayMessage {
                    role: MessageRole::User,
                    content: labels.join(", "),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.invalidate_chat_render_cache();
                // Send first selected index for single select
                let _ = tx.send(indices.first().copied());
            }
            (AskResult::Text(text), Some(AskReply::Select(tx))) => {
                // User typed custom text on a Select ask.
                // Find if the text matches an option label (case-insensitive).
                let match_idx = state
                    .options
                    .iter()
                    .position(|o| o.label.eq_ignore_ascii_case(text));
                if let Some(idx) = match_idx {
                    self.messages.push(DisplayMessage {
                        role: MessageRole::User,
                        content: state.options[idx].label.clone(),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                    self.invalidate_chat_render_cache();
                    let _ = tx.send(Some(idx));
                } else if let Some(other_idx) = state
                    .options
                    .iter()
                    .position(|o| o.label.eq_ignore_ascii_case("Other..."))
                {
                    // Select-style UI replies can only send an index back to the
                    // tool. Preserve the user's typed custom answer visually, then
                    // select the explicit Other option so the ask_user tool can
                    // collect and return free text instead of treating the answer
                    // as skipped.
                    self.messages.push(DisplayMessage {
                        role: MessageRole::User,
                        content: text.clone(),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                    self.invalidate_chat_render_cache();
                    let _ = tx.send(Some(other_idx));
                } else {
                    // No match and no explicit Other option — send None. The ask
                    // tool will report the question as skipped.
                    self.messages.push(DisplayMessage {
                        role: MessageRole::User,
                        content: text.clone(),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                    self.invalidate_chat_render_cache();
                    let _ = tx.send(None);
                }
            }
            (AskResult::Selected(indices), Some(AskReply::SelectOrInput(tx))) => {
                let index = indices.first().copied();
                if let Some(index) = index {
                    if let Some(option) = state.options.get(index) {
                        self.messages.push(DisplayMessage {
                            role: MessageRole::User,
                            content: option.label.clone(),
                            thinking: None,
                            tool_calls: Vec::new(),
                            assistant_blocks: Vec::new(),
                            is_streaming: false,
                            timestamp: imp_llm::now(),
                        });
                    }
                }
                self.invalidate_chat_render_cache();
                let _ = tx.send(index.map(SelectionAnswer::Choice));
            }
            (AskResult::Text(text), Some(AskReply::SelectOrInput(tx))) => {
                let match_idx = state
                    .options
                    .iter()
                    .position(|o| o.label.eq_ignore_ascii_case(text));
                self.messages.push(DisplayMessage {
                    role: MessageRole::User,
                    content: text.clone(),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.invalidate_chat_render_cache();
                let answer = match_idx
                    .map(SelectionAnswer::Choice)
                    .unwrap_or_else(|| SelectionAnswer::Text(text.clone()));
                let _ = tx.send(Some(answer));
            }
            (AskResult::Selected(indices), Some(AskReply::MultiSelect(tx))) => {
                let labels: Vec<String> = indices
                    .iter()
                    .filter_map(|&i| state.options.get(i).map(|o| o.label.clone()))
                    .collect();
                self.messages.push(DisplayMessage {
                    role: MessageRole::User,
                    content: labels.join(", "),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.invalidate_chat_render_cache();
                let _ = tx.send(Some(indices.clone()));
            }
            (AskResult::Text(text), Some(AskReply::MultiSelect(tx))) => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::User,
                    content: text.clone(),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.invalidate_chat_render_cache();
                let indices: Vec<usize> = state
                    .options
                    .iter()
                    .enumerate()
                    .filter_map(|(index, option)| {
                        option.label.eq_ignore_ascii_case(text).then_some(index)
                    })
                    .collect();
                let _ = tx.send((!indices.is_empty()).then_some(indices));
            }
            _ => {}
        }
    }

    pub(super) fn advance_secrets_flow(&mut self, input: Option<String>) {
        let Some(flow) = self.secrets_flow.take() else {
            return;
        };

        match flow {
            SecretsFlowState::AwaitingFieldNames { provider } => {
                let field_names = parse_secret_field_names(input.as_deref().unwrap_or(""));
                let first_field = field_names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "api_key".into());
                self.secrets_flow = Some(SecretsFlowState::AwaitingFieldValues {
                    provider,
                    fields: field_names,
                    current: 0,
                    values: HashMap::new(),
                });
                let (tx, _rx) = tokio::sync::oneshot::channel();
                self.begin_ask(
                    crate::views::ask_bar::AskState::new(
                        format!("Enter {first_field}:"),
                        String::new(),
                        vec![],
                        false,
                    ),
                    AskReply::Input(tx),
                );
            }
            SecretsFlowState::AwaitingFieldValues {
                provider,
                fields,
                current,
                mut values,
            } => {
                let Some(value) = input.filter(|value| !value.trim().is_empty()) else {
                    self.push_error_msg("Secret entry cancelled.");
                    return;
                };

                let field = fields
                    .get(current)
                    .cloned()
                    .unwrap_or_else(|| "api_key".into());
                values.insert(field, value.trim().to_string());

                if current + 1 < fields.len() {
                    let next_field = fields[current + 1].clone();
                    self.secrets_flow = Some(SecretsFlowState::AwaitingFieldValues {
                        provider: provider.clone(),
                        fields: fields.clone(),
                        current: current + 1,
                        values,
                    });
                    let (tx, _rx) = tokio::sync::oneshot::channel();
                    self.begin_ask(
                        crate::views::ask_bar::AskState::new(
                            format!("Enter {next_field}:"),
                            String::new(),
                            vec![],
                            false,
                        ),
                        AskReply::Input(tx),
                    );
                    return;
                }

                let auth_path = imp_core::storage::global_auth_path();
                let mut auth_store = AuthStore::load(&auth_path)
                    .unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
                match auth_store.store_secret_fields(&provider, values) {
                    Ok(()) => {
                        self.push_system_msg(&format!("Saved secure secrets for {provider}."))
                    }
                    Err(e) => {
                        self.push_error_msg(&format!("Failed to save secrets for {provider}: {e}"))
                    }
                }
            }
        }
    }

    pub(super) fn cancel_ask(&mut self) {
        self.secrets_flow = None;
        self.ask_state = None;
        self.restore_editor_after_ask();
        if let Some(reply) = self.ask_reply.take() {
            match reply {
                AskReply::Select(tx) => {
                    let _ = tx.send(None);
                }
                AskReply::SelectOrInput(tx) => {
                    let _ = tx.send(None);
                }
                AskReply::MultiSelect(tx) => {
                    let _ = tx.send(None);
                }
                AskReply::Input(tx) => {
                    let _ = tx.send(None);
                }
            }
        }
        // Stop the agent — user wants control back
        if let Some(ref handle) = self.agent_handle {
            let _ = handle.command_tx.try_send(AgentCommand::Cancel);
        }
        self.is_streaming = false;
    }
}
