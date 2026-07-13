use std::sync::Arc;

use imp_core::compaction::checkpoint::{checkpoint_source, CheckpointStore};
use imp_core::compaction::coordinator::{generate_checkpoint, CheckpointRequest};
use imp_llm::auth::AuthStore;
use imp_llm::model::Model;
use imp_llm::providers::create_provider;

use super::model_auth::{resolve_provider_api_key, should_use_chatgpt_provider};
use super::App;

impl App {
    pub(super) fn schedule_checkpoint_if_due(&mut self) {
        if self.checkpoint_task.is_some() || self.is_streaming {
            return;
        }
        let Some(session_path) = self.session.path() else {
            return;
        };
        let store = CheckpointStore::for_session(session_path);
        let active = self.session.get_active_message_entries();
        let mut previous = match store.load() {
            Ok(previous) => previous,
            Err(error) => {
                self.push_warning_msg(&format!("Compaction checkpoint skipped: {error}"));
                return;
            }
        };
        let source = match checkpoint_source(&active, previous.as_ref()) {
            Ok(source) => source,
            Err(_) => {
                previous = None;
                match checkpoint_source(&active, None) {
                    Ok(source) => source,
                    Err(error) => {
                        self.push_warning_msg(&format!("Compaction checkpoint skipped: {error}"));
                        return;
                    }
                }
            }
        };
        let config = self.config.context.summarizer.clone();
        if !source.is_due(config.checkpoint_interval_tokens) {
            return;
        }
        let model_id = if config.model.trim() == "default" {
            self.model_name.as_str()
        } else {
            config.model.trim()
        };
        let mut meta = match self.model_registry.resolve_meta(model_id, None) {
            Some(meta) => meta,
            None => {
                self.push_warning_msg(&format!("Unknown compaction model: {model_id}"));
                return;
            }
        };
        let auth_path = imp_core::storage::global_auth_path();
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
        let mut provider_name = meta.provider.clone();
        if should_use_chatgpt_provider(&auth_store, &self.model_registry, &meta) {
            provider_name = "openai-codex".to_string();
            let Some(routed) = self
                .model_registry
                .resolve_meta(model_id, Some(&provider_name))
            else {
                self.push_warning_msg(&format!(
                    "Compaction model {model_id} is unavailable through {provider_name}"
                ));
                return;
            };
            meta = routed;
        }
        let Some(provider) = create_provider(&provider_name) else {
            self.push_warning_msg(&format!("Unknown compaction provider: {provider_name}"));
            return;
        };
        self.status_items.insert(
            "compaction-checkpoint".into(),
            format!("Checkpointing with {model_id}…"),
        );
        self.checkpoint_task = Some(tokio::spawn(async move {
            let api_key = resolve_provider_api_key(&mut auth_store, &provider_name)
                .await
                .map_err(|error| error.to_string())?;
            generate_checkpoint(CheckpointRequest {
                active,
                previous,
                store,
                model: Model {
                    meta,
                    provider: Arc::from(provider),
                },
                api_key,
                config,
                authoritative_state: None,
            })
            .await
            .map_err(|error| error.to_string())
        }));
    }
}
