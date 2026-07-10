use crossterm::event::{KeyCode, KeyEvent};
use imp_llm::auth::AuthStore;

use crate::theme::Theme;
use crate::views::settings::{SettingsField, SettingsState};

use super::{provider_logged_in, App, DisplayMessage, MessageRole, RuntimeSignal, UiMode};

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
                } else {
                    let is_browser_health = matches!(
                        &self.mode,
                        UiMode::Settings(state)
                            if state.current_field() == SettingsField::BrowserHealth
                    );
                    let is_browser_install = matches!(
                        &self.mode,
                        UiMode::Settings(state)
                            if state.current_field() == SettingsField::BrowserInstall
                    );
                    if is_browser_health {
                        self.run_browser_diagnostics();
                    } else if is_browser_install {
                        self.confirm_browser_install();
                    } else if let UiMode::Settings(ref mut state) = self.mode {
                        state.start_edit();
                    }
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

    fn confirm_browser_install(&mut self) {
        let configured_binary = match &self.mode {
            UiMode::Settings(settings) => settings.browser.config().binary,
            _ => None,
        };
        if configured_binary.is_some() {
            self.push_error_msg(
                "Clear the explicit Lightpanda binary path before package-manager installation.",
            );
            return;
        }
        if imp_core::tools::browser::resolve_lightpanda_binary(None).is_ok() {
            self.push_system_msg("Lightpanda is already installed. Run diagnostics to verify it.");
            return;
        }
        let plan = match imp_core::tools::browser::browser_install_plan() {
            Ok(plan) => plan,
            Err(error) => {
                self.push_error_msg(&error);
                return;
            }
        };
        let command = plan.command_display();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.handle_ui_request(crate::tui_interface::UiRequest::Confirm {
            title: "Install Lightpanda".into(),
            message: format!("Run `{command}`?"),
            reply: reply_tx,
        });
        let signal_tx = self.runtime_signal_tx.clone();
        tokio::spawn(async move {
            let confirmed = reply_rx.await.ok().flatten().unwrap_or(false);
            if !confirmed {
                return;
            }
            let result = imp_core::tools::browser::install_browser(&plan).await;
            let _ = signal_tx.send(RuntimeSignal::BrowserInstallCompleted(result));
        });
    }

    pub(super) fn run_browser_diagnostics(&mut self) {
        let UiMode::Settings(settings) = &mut self.mode else {
            return;
        };
        if matches!(
            settings.browser.health,
            crate::views::settings::browser::BrowserHealth::Checking
        ) {
            return;
        }
        settings.browser.health = crate::views::settings::browser::BrowserHealth::Checking;
        let config = settings.browser.config();
        let signal_tx = self.runtime_signal_tx.clone();
        tokio::spawn(async move {
            let report = imp_core::tools::browser::diagnose_browser(&config).await;
            let _ = signal_tx.send(RuntimeSignal::BrowserDiagnosticCompleted(report));
        });
    }

    pub(super) fn save_settings(&mut self) {
        // Extract state before mutating self
        let state = match &self.mode {
            UiMode::Settings(s) => s.clone(),
            _ => return,
        };

        // Apply to in-session config
        let mut next_config = self.config.clone();
        state.apply_to_config(&mut next_config);
        if let Err(error) = state.browser.validate() {
            self.push_system_msg(&format!("Browser settings invalid: {error}"));
            return;
        }
        self.config = next_config;
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
