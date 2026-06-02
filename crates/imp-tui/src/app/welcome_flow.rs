use crossterm::event::{KeyCode, KeyEvent};
use imp_llm::auth::AuthStore;

use super::{App, DisplayMessage, MessageRole, UiMode, WelcomeAuthMethod, WelcomeStep};

impl App {
    pub(super) fn handle_welcome_key(&mut self, key: KeyEvent) {
        let step = match &self.mode {
            UiMode::Welcome(s) => s.current_step(),
            _ => return,
        };

        match step {
            WelcomeStep::Welcome | WelcomeStep::ProviderAuth => match key.code {
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.toggle_auth_method();
                    }
                }
                KeyCode::Char('o') | KeyCode::Char('O') => {
                    let provider = if let UiMode::Welcome(ref mut state) = self.mode {
                        state.select_oauth_method();
                        if state.selected_provider_supports_oauth() {
                            state.mark_oauth_pending();
                            state.selected_provider_id().map(str::to_string)
                        } else {
                            state.set_key_error(
                                "OAuth is not available for this provider. Paste an API key instead.",
                            );
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(provider) = provider {
                        self.start_login(&provider);
                    }
                }
                KeyCode::Up => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.provider_up();
                        let all_models = self.model_registry.list().to_vec();
                        state.update_models(&all_models);
                    }
                }
                KeyCode::Down => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.provider_down();
                        let all_models = self.model_registry.list().to_vec();
                        state.update_models(&all_models);
                    }
                }
                KeyCode::Enter => {
                    let maybe_provider = if let UiMode::Welcome(ref mut state) = self.mode {
                        match state.auth_method() {
                            WelcomeAuthMethod::OAuth => {
                                if state.selected_provider_supports_oauth() {
                                    state.mark_oauth_pending();
                                    state.selected_provider_id().map(str::to_string)
                                } else {
                                    state.set_key_error(
                                        "OAuth is not available for this provider. Paste an API key instead.",
                                    );
                                    None
                                }
                            }
                            WelcomeAuthMethod::ApiKey => match state.check_auth_resolved() {
                                Ok(()) => {
                                    state.advance();
                                    None
                                }
                                Err(error) => {
                                    state.set_key_error(error);
                                    None
                                }
                            },
                        }
                    } else {
                        None
                    };
                    if let Some(provider) = maybe_provider {
                        self.start_login(&provider);
                    }
                }
                KeyCode::Esc => {
                    if matches!(step, WelcomeStep::Welcome) {
                        self.mode = UiMode::Normal;
                    } else if let UiMode::Welcome(ref mut state) = self.mode {
                        state.go_back();
                    }
                }
                KeyCode::Backspace => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.select_api_key_method();
                        state.pop_key_char();
                    }
                }
                KeyCode::Char(c) => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.select_api_key_method();
                        state.push_key_char(c);
                    }
                }
                _ => {}
            },
            WelcomeStep::ModelThinking => match key.code {
                KeyCode::Up => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.model_up();
                    }
                }
                KeyCode::Down => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.model_down();
                    }
                }
                KeyCode::Right => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.cycle_thinking();
                    }
                }
                KeyCode::Left => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.cycle_thinking_back();
                    }
                }
                KeyCode::Enter => {
                    self.finish_welcome();
                }
                KeyCode::Esc => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.go_back();
                    }
                }
                _ => {}
            },
            WelcomeStep::WebSearch => match key.code {
                KeyCode::Up => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.web_provider_up();
                    }
                }
                KeyCode::Down => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.web_provider_down();
                    }
                }
                KeyCode::Enter => {
                    let web_result = if let UiMode::Welcome(ref mut state) = self.mode {
                        state.check_web_auth_resolved()
                    } else {
                        Ok(())
                    };
                    match web_result {
                        Ok(()) => {
                            self.finish_welcome();
                        }
                        Err(error) => {
                            self.messages.push(DisplayMessage {
                                role: MessageRole::Error,
                                content: error,
                                thinking: None,
                                tool_calls: Vec::new(),
                                assistant_blocks: Vec::new(),
                                is_streaming: false,
                                timestamp: imp_llm::now(),
                            });
                        }
                    }
                }
                KeyCode::Esc => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.go_back();
                    }
                }
                KeyCode::Backspace => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.pop_web_key_char();
                    }
                }
                KeyCode::Char(c) => {
                    if let UiMode::Welcome(ref mut state) = self.mode {
                        state.push_web_key_char(c);
                    }
                }
                _ => {}
            },
            WelcomeStep::Done => match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.mode = UiMode::Normal;
                }
                _ => {}
            },
        }
    }

    /// Persist welcome flow choices to config and auth, then advance to Done step.
    pub(super) fn finish_welcome(&mut self) {
        let (
            model_id,
            thinking,
            provider_id,
            resolved_key,
            resolved_web_provider,
            resolved_web_key,
        ) = match &self.mode {
            UiMode::Welcome(state) => {
                let model_id = state
                    .selected_model()
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| "claude-sonnet-4-6".to_string());
                let thinking = state.thinking_level;
                let provider_id = state
                    .selected_provider_id()
                    .unwrap_or("anthropic")
                    .to_string();
                let resolved_key = state.resolved_key.clone();
                let resolved_web_provider = state.resolved_web_provider.clone();
                let resolved_web_key = state.resolved_web_key.clone();
                (
                    model_id,
                    thinking,
                    provider_id,
                    resolved_key,
                    resolved_web_provider,
                    resolved_web_key,
                )
            }
            _ => return,
        };

        // Update in-session config
        self.config.model = Some(model_id.clone());
        self.config.thinking = Some(thinking);
        self.model_name = model_id;
        self.thinking_level = thinking;

        if let Some(meta) = self.model_registry.resolve_meta(&self.model_name, None) {
            self.context_window = meta.context_window;
        }

        if let Some(web_provider) = resolved_web_provider
            .as_deref()
            .filter(|provider| *provider != "none")
        {
            self.config.web.search_provider = match web_provider {
                "tavily" => Some(imp_core::tools::web::types::SearchProvider::Tavily),
                "exa" => Some(imp_core::tools::web::types::SearchProvider::Exa),
                "linkup" => Some(imp_core::tools::web::types::SearchProvider::Linkup),
                "perplexity" => Some(imp_core::tools::web::types::SearchProvider::Perplexity),
                _ => self.config.web.search_provider,
            };
            std::env::set_var("IMP_WEB_PROVIDER", web_provider);
        }

        // Save config.toml
        let config_path = imp_core::storage::global_config_path();
        if let Err(e) = self.config.save(&config_path) {
            self.messages.push(DisplayMessage {
                role: MessageRole::Error,
                content: format!("Failed to save config: {e}"),
                thinking: None,
                tool_calls: Vec::new(),
                assistant_blocks: Vec::new(),
                is_streaming: false,
                timestamp: imp_llm::now(),
            });
        }

        let auth_path = imp_core::storage::global_auth_path();
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

        // Save API key if one was manually entered
        if let Some(key) = resolved_key {
            if let Err(e) = auth_store.store(
                &provider_id,
                imp_llm::auth::StoredCredential::ApiKey { key },
            ) {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!("Failed to save API key: {e}"),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
            }
        }

        if let (Some(web_provider), Some(web_key)) = (
            resolved_web_provider
                .as_deref()
                .filter(|provider| *provider != "none"),
            resolved_web_key,
        ) {
            if let Err(e) = auth_store.store(
                web_provider,
                imp_llm::auth::StoredCredential::ApiKey { key: web_key },
            ) {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!("Failed to save web API key: {e}"),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
            }
        }

        // Advance to Done screen
        if let UiMode::Welcome(ref mut state) = self.mode {
            state.advance();
        }
    }
}
