use super::*;

impl App {
    pub(in crate::app) fn start_secrets_flow(&mut self, provider: &str) {
        self.mode = UiMode::Normal;
        self.secrets_flow = Some(SecretsFlowState::AwaitingFieldNames {
            provider: provider.to_string(),
        });
        let (tx, _rx) = tokio::sync::oneshot::channel();
        self.begin_ask(
            crate::views::ask_bar::AskState::new(
                format!(
                    "{}\n\nField names (comma-separated) [api_key]:",
                    prompt_text_for_secret_provider(provider)
                ),
                String::new(),
                vec![],
                false,
            ),
            AskReply::Input(tx),
        );
    }

    pub(in crate::app) fn start_login(&mut self, provider: &str) {
        if !oauth_provider(provider) {
            self.push_error_msg(&format!(
                "/login {provider} is OAuth-only. Use /secrets {provider} for API keys/secrets."
            ));
            return;
        }

        let status_message = match provider {
            "anthropic" => "Opening browser for Anthropic login...",
            "openai" | "openai-codex" => "Opening browser for OpenAI / ChatGPT login...",
            "kimi-code" => "Opening browser for Kimi Code login...",
            _ => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!(
                        "OAuth login for '{provider}' not supported. Use /secrets {provider} for API keys."
                    ),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                return;
            }
        };

        let in_welcome = matches!(self.mode, UiMode::Welcome(_));
        if !in_welcome {
            self.mode = UiMode::Normal;
            self.push_system_msg(status_message);
        }

        let auth_path = imp_core::storage::global_auth_path();
        let provider = provider.to_string();
        let signal_tx = self.runtime_signal_tx.clone();
        let task = tokio::spawn(async move {
            let provider_for_url = provider.clone();
            let url_signal_tx = signal_tx.clone();
            let emit_url = move |url: &str| {
                let browser_opened = open_url(url);
                let _ = url_signal_tx.send(RuntimeSignal::LoginUrlReady {
                    provider: provider_for_url.clone(),
                    url: url.to_string(),
                    browser_opened,
                });
            };
            let login_result = match provider.as_str() {
                "anthropic" => {
                    imp_llm::oauth::anthropic::AnthropicOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            || async { None },
                        )
                        .await
                }
                "openai" | "openai-codex" => {
                    imp_llm::oauth::chatgpt::ChatGptOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            || async { None },
                        )
                        .await
                }
                "kimi-code" => {
                    imp_llm::oauth::kimi_code::KimiCodeOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            |_msg| {
                                // Messages are silently dropped in the TUI background task;
                                // the browser URL is the primary signal.
                            },
                        )
                        .await
                }
                _ => unreachable!(),
            };

            match login_result {
                Ok(credential) => {
                    let success_message = imp_llm::auth::oauth_display_info_for_credential(
                        provider.as_str(),
                        &credential,
                    )
                    .map(|info| info.login_message(provider.as_str()))
                    .unwrap_or_else(|| format!("Logged in to {} successfully.", provider));

                    let mut store = AuthStore::load(&auth_path)
                        .unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
                    match provider.as_str() {
                        "anthropic" => {
                            let _ = store.store(
                                "anthropic",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        "openai" | "openai-codex" => {
                            let _ = store.store(
                                "openai",
                                imp_llm::auth::StoredCredential::OAuth(credential.clone()),
                            );
                            let _ = store.store(
                                "openai-codex",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        "kimi-code" => {
                            let _ = store.store(
                                "kimi-code",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        _ => {}
                    }
                    LoginTaskExit::Success(success_message)
                }
                Err(e) => LoginTaskExit::Failed(format!("OAuth login failed: {e}")),
            }
        });
        self.login_task = Some(task);
    }

    pub(in crate::app) fn open_secrets_picker(&mut self) {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let providers = secret_providers(&ProviderRegistry::with_builtins())
            .into_iter()
            .map(|mut provider| {
                provider.configured = provider_logged_in(&auth_store, &provider.id);
                provider
            })
            .collect();
        self.mode = UiMode::SecretsPicker(SecretsPickerState::new(providers));
    }

    pub(in crate::app) fn open_login_picker(&mut self) {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let providers = login_providers(&ProviderRegistry::with_builtins())
            .into_iter()
            .filter(|provider| oauth_provider(provider.id))
            .map(|mut provider| {
                provider.logged_in = provider_logged_in(&auth_store, provider.id);
                provider
            })
            .collect();
        self.mode = UiMode::LoginPicker(LoginPickerState::new(providers));
    }
}
