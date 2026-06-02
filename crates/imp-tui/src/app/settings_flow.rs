use crossterm::event::{KeyCode, KeyEvent};
use imp_llm::auth::AuthStore;

use crate::theme::Theme;
use crate::views::settings::{SettingsField, SettingsState};

use super::{provider_logged_in, App, DisplayMessage, MessageRole, UiMode};

impl App {
    pub(super) fn open_settings(&mut self) {
        let models = self.filtered_models();
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let state = SettingsState::new(&self.config, &self.model_name, &models, &auth_store);
        self.mode = UiMode::Settings(state);
    }

    pub(super) fn handle_settings_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Commit any pending edit, then dismiss
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.commit_edit();
                }
                self.mode = UiMode::Normal;
            }
            KeyCode::Up => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.move_up();
                }
            }
            KeyCode::Down => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.move_down();
                }
            }
            KeyCode::Tab => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.switch_tab_forward();
                }
            }
            KeyCode::BackTab => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.switch_tab_backward();
                }
            }
            KeyCode::Left => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.cycle_backward();
                }
            }
            KeyCode::Right => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.cycle_forward();
                }
            }
            KeyCode::Enter => {
                let is_save = matches!(
                    &self.mode,
                    UiMode::Settings(s) if s.current_field() == SettingsField::Save
                );
                if is_save {
                    self.save_settings();
                } else if let UiMode::Settings(ref mut state) = self.mode {
                    state.start_edit();
                }
            }
            KeyCode::Backspace => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.pop_char();
                }
            }
            KeyCode::Char(c) => {
                if let UiMode::Settings(ref mut state) = self.mode {
                    state.push_char(c);
                }
            }
            _ => {}
        }
    }

    pub(super) fn save_settings(&mut self) {
        // Extract state before mutating self
        let state = match &self.mode {
            UiMode::Settings(s) => s.clone(),
            _ => return,
        };

        // Apply to in-session config
        state.apply_to_config(&mut self.config);
        self.model_name = state.model.clone();
        self.thinking_level = state.thinking_level;
        self.theme = Theme::named(self.config.theme.as_deref().unwrap_or("default"));

        // Update context window from registry
        if let Some(meta) = self.model_registry.resolve_meta(&self.model_name, None) {
            self.context_window = meta.context_window;
        }

        let auth_path = imp_core::storage::global_auth_path();
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let mut auth_notes = Vec::new();

        for (provider, value) in [
            ("tavily", state.tavily_api_key.trim()),
            ("exa", state.exa_api_key.trim()),
        ] {
            if value.is_empty() {
                continue;
            }

            match auth_store.store(
                provider,
                imp_llm::auth::StoredCredential::ApiKey {
                    key: value.to_string(),
                },
            ) {
                Ok(()) => auth_notes.push(format!("saved {provider} key")),
                Err(e) => {
                    self.messages.push(DisplayMessage {
                        role: MessageRole::Error,
                        content: format!("Failed to save {provider} API key: {e}"),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                }
            }
        }

        // Persist to user config.toml
        let config_path = imp_core::storage::global_config_path();
        match self.config.save(&config_path) {
            Ok(()) => {
                if let UiMode::Settings(ref mut s) = self.mode {
                    s.dirty = false;
                    s.tavily_api_key.clear();
                    s.exa_api_key.clear();
                    s.tavily_configured = provider_logged_in(&auth_store, "tavily");
                    s.exa_configured = provider_logged_in(&auth_store, "exa");
                }
                let mut message = format!("Settings saved to {}", config_path.display());
                if !auth_notes.is_empty() {
                    message.push_str(&format!(" ({})", auth_notes.join(", ")));
                }
                self.messages.push(DisplayMessage {
                    role: MessageRole::System,
                    content: message,
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
            }
            Err(e) => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!("Failed to save settings: {e}"),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
            }
        }
    }
}
