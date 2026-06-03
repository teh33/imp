use std::sync::Arc;

use imp_core::builder::AgentBuilder;
use imp_core::compaction::{
    execute_compaction_with_retry, execute_manual_compaction, prepare_messages_for_compaction,
    select_compaction_strategy, CompactionCapabilities, CompactionStrategy,
    COMPACTION_SUMMARY_PREFIX, DEFAULT_KEEP_RECENT_GROUPS, LOCAL_COMPACTION_KEEP_RECENT_GROUPS,
};
use imp_core::session::SessionManager;
use imp_core::Error as ImpCoreError;
use imp_llm::auth::AuthStore;
use imp_llm::providers::create_provider;
use imp_llm::{Message, Model, StreamEvent};

use crate::views::chat::{DisplayMessage, MessageRole};

use super::{resolve_provider_api_key, should_use_chatgpt_provider, App};

impl App {
    pub(super) fn run_manual_compaction(&mut self, summarize: bool) {
        if self.is_streaming {
            self.push_error_msg("Cannot compact while the agent is actively streaming.");
            return;
        }
        if self.compaction_task.is_some() {
            self.push_system_msg("Compaction is already running.");
            return;
        }

        let active_messages = self.session.get_active_messages();
        let prepared =
            prepare_messages_for_compaction(&active_messages, DEFAULT_KEEP_RECENT_GROUPS);
        if !prepared.should_compact() {
            self.push_system_msg("Not enough history to compact yet.");
            return;
        }

        if !summarize {
            self.finish_manual_compaction(String::new());
            return;
        }

        let auth_path = imp_core::storage::global_auth_path();
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

        let mut meta = match self.model_registry.resolve_meta(&self.model_name, None) {
            Some(meta) => meta,
            None => {
                self.push_error_msg(&format!("Unknown model: {}", self.model_name));
                return;
            }
        };

        let mut provider_name = meta.provider.clone();
        if should_use_chatgpt_provider(&auth_store, &self.model_registry, &meta) {
            provider_name = "openai-codex".to_string();
            if let Some(resolved) = self
                .model_registry
                .resolve_meta(&self.model_name, Some(&provider_name))
            {
                meta = resolved;
            }
        }

        let provider = match create_provider(&provider_name) {
            Some(provider) => provider,
            None => {
                self.push_error_msg(&format!("Unknown provider: {provider_name}"));
                return;
            }
        };

        let model = Model {
            meta,
            provider: Arc::from(provider),
        };
        let model_id = model.meta.id.clone();
        let model_meta = model.meta.clone();
        let model_provider = Arc::clone(&model.provider);
        let requested_max_tokens = self.config.max_tokens;
        let thinking_level = self.thinking_level;

        let mut config = self.config.clone();
        config.thinking = Some(thinking_level);

        let strategy = select_compaction_strategy(&CompactionCapabilities {
            provider_id: &provider_name,
            model_id: &model_id,
            allow_provider_native: false,
        });
        if matches!(strategy, CompactionStrategy::ProviderNative) {
            self.push_system_msg(
                "Provider-native compaction is not enabled yet; falling back to local compaction.",
            );
        }

        self.messages.push(DisplayMessage {
            role: MessageRole::Compaction,
            content: "Compacting context…".to_string(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: true,
            timestamp: imp_llm::now(),
        });
        self.auto_scroll = true;
        self.scroll_offset = 0;
        self.invalidate_chat_render_cache();

        let cwd = self.cwd.clone();
        let lua_cwd = self.cwd.clone();
        let user_config_dir = imp_core::config::Config::user_config_dir();
        let task = tokio::spawn(async move {
            let api_key = resolve_provider_api_key(&mut auth_store, &provider_name)
                .await
                .map_err(|e| format!("Failed to resolve auth for compaction: {e}"))?;

            let model = Model {
                meta: model_meta.clone(),
                provider: Arc::clone(&model_provider),
            };
            let (agent, _handle) = AgentBuilder::new(config, cwd, model, api_key)
                .lua_tool_loader(move |policy, tools| {
                    imp_lua::init_lua_extensions(&user_config_dir, Some(&lua_cwd), tools, policy);
                })
                .build()
                .map_err(|e| format!("Failed to build compaction agent: {e}"))?;

            let system_prompt = agent.system_prompt.clone();
            let retry_policy = agent.retry_policy.clone();
            execute_compaction_with_retry(
                &mut SessionManager::in_memory_with_messages(active_messages),
                DEFAULT_KEEP_RECENT_GROUPS,
                2,
                |prompt| {
                    use futures::StreamExt;
                    use imp_llm::provider::{CacheOptions, Context as LlmContext, RequestOptions};

                    let model_meta = model_meta.clone();
                    let model_provider = Arc::clone(&model_provider);
                    let api_key = agent.api_key.clone();
                    let system_prompt = system_prompt.clone();
                    let prompt = prompt.to_string();
                    let retry_policy = retry_policy.clone();
                    let prompt_tokens = imp_core::context::estimate_tokens(&prompt);
                    let prompt_limit =
                        ((model_meta.context_window as f64) * 0.6).floor().max(1.0) as u32;
                    if prompt_tokens > prompt_limit {
                        return Ok(None);
                    }

                    futures::executor::block_on(async move {
                        let mut summary = String::new();
                        let mut message_end_text: Option<String> = None;
                        let model = Model {
                            meta: model_meta,
                            provider: model_provider,
                        };
                        let context = LlmContext {
                            messages: vec![Message::user(prompt)],
                            session_id: None,
                            thread_id: None,
                        };
                        let compaction_max_tokens = requested_max_tokens
                            .map(|tokens| tokens.min(4096))
                            .or(Some(2048));
                        let options = RequestOptions {
                            thinking_level,
                            max_tokens: compaction_max_tokens,
                            temperature: Some(0.2),
                            system_prompt,
                            tools: Vec::new(),
                            cache_options: CacheOptions::default(),
                            effort: None,
                        };

                        let mut stream = imp_core::retry::stream_with_retry(
                            move || {
                                model.provider.stream(
                                    &model,
                                    context.clone(),
                                    options.clone(),
                                    &api_key,
                                )
                            },
                            retry_policy,
                        );

                        let stream_result =
                            tokio::time::timeout(std::time::Duration::from_secs(180), async {
                                while let Some(item) = stream.next().await {
                                    match item {
                                        Ok(StreamEvent::TextDelta { text }) => {
                                            summary.push_str(&text)
                                        }
                                        Ok(StreamEvent::MessageEnd { message }) => {
                                            let body = message
                                                .content
                                                .iter()
                                                .filter_map(|block| match block {
                                                    imp_llm::ContentBlock::Text { text } => {
                                                        Some(text.as_str())
                                                    }
                                                    _ => None,
                                                })
                                                .collect::<Vec<_>>()
                                                .join("");
                                            if !body.is_empty() {
                                                message_end_text = Some(body);
                                            }
                                        }
                                        Ok(_) => {}
                                        Err(error) => return Err(error.to_string()),
                                    }
                                }
                                Ok::<(), String>(())
                            })
                            .await;

                        match stream_result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => return Err(error),
                            Err(_) => {
                                return Ok(None);
                            }
                        }

                        let final_text = if !summary.trim().is_empty() {
                            summary
                        } else {
                            message_end_text.unwrap_or_default()
                        };
                        if final_text.trim().is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(final_text))
                        }
                    })
                    .map_err(|error| ImpCoreError::Llm(imp_llm::Error::Provider(error)))
                },
            )
            .map_err(|e| e.to_string())?
            .map(|result| {
                result
                    .summary
                    .trim_start_matches(COMPACTION_SUMMARY_PREFIX)
                    .to_string()
            })
            .ok_or_else(|| "Not enough history to compact yet.".to_string())
        });

        self.compaction_task = Some(task);
    }

    pub(super) fn finish_compaction_status_message(&mut self, content: &str) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == MessageRole::Compaction && message.is_streaming)
        {
            message.content = content.to_string();
            message.is_streaming = false;
            self.invalidate_chat_render_cache();
        }
    }

    pub(super) fn finish_lua_command_status_message(&mut self, content: &str) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == MessageRole::Compaction && message.is_streaming)
        {
            message.content = content.to_string();
            message.is_streaming = false;
            self.invalidate_chat_render_cache();
        }
    }

    pub(super) fn finish_manual_compaction(&mut self, summary: String) {
        let keep_recent_groups = if summary.trim().is_empty() {
            LOCAL_COMPACTION_KEEP_RECENT_GROUPS
        } else {
            DEFAULT_KEEP_RECENT_GROUPS
        };
        let result = execute_manual_compaction(&mut self.session, keep_recent_groups, |_| {
            if summary.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(summary.clone()))
            }
        });

        match result {
            Ok(Some(compaction)) => {
                self.load_session_messages();
                self.messages.push(DisplayMessage {
                    role: MessageRole::Compaction,
                    content: format!(
                        "Context compacted. Saved ~{} tokens. Preserved recent working context.",
                        compaction
                            .tokens_before
                            .saturating_sub(compaction.tokens_after)
                    ),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                self.push_system_msg(
                    "Compaction summary stored. Active context now uses the compacted branch view.",
                );
            }
            Ok(None) => {
                self.finish_compaction_status_message("Not enough history to compact yet.");
            }
            Err(e) => {
                self.finish_compaction_status_message("Compaction failed.");
                self.push_error_msg(&format!("Compaction failed: {e}"));
            }
        }
    }
}
