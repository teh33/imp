use std::sync::Arc;

use imp_core::compaction::checkpoint::{
    checkpoint_source_for_model, CheckpointStore, CompactionCheckpoint,
};
use imp_core::compaction::coordinator::{
    generate_checkpoint, CheckpointGenerationMode, CheckpointRequest,
};
use imp_core::session::ActiveSessionMessage;
use imp_llm::auth::AuthStore;
use imp_llm::model::{Model, ModelMeta};
use imp_llm::providers::create_provider;

use super::model_auth::{resolve_provider_api_key, should_use_chatgpt_provider};
use super::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckpointPurpose {
    Rolling,
    ManualCompaction,
}

pub(super) struct CheckpointTask {
    pub(super) handle: tokio::task::JoinHandle<Result<(), String>>,
    pub(super) purpose: CheckpointPurpose,
}

impl App {
    pub(super) fn schedule_checkpoint_if_due(&mut self) {
        if self.session.path().is_none() {
            return;
        }
        if let Err(error) = self.start_checkpoint(
            CheckpointGenerationMode::WhenDue,
            CheckpointPurpose::Rolling,
        ) {
            self.push_warning_msg(&format!("Compaction checkpoint skipped: {error}"));
        }
    }

    pub(super) fn start_manual_checkpoint(&mut self) -> Result<bool, String> {
        self.start_checkpoint(
            CheckpointGenerationMode::Force,
            CheckpointPurpose::ManualCompaction,
        )
    }

    fn start_checkpoint(
        &mut self,
        generation_mode: CheckpointGenerationMode,
        purpose: CheckpointPurpose,
    ) -> Result<bool, String> {
        if self.checkpoint_task.is_some() || self.is_streaming {
            return Ok(false);
        }
        let session_path = self.compaction_session_path()?;
        let store = CheckpointStore::for_session(session_path);
        let active = self.session.get_active_message_entries();
        let previous = store.load().map_err(|error| error.to_string())?;
        let config = self.config.context.summarizer.clone();
        let model_id = self.compaction_model_id(&config.model).to_string();
        let auth_path = imp_core::storage::global_auth_path();
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
        let (meta, provider_name) = self.resolve_checkpoint_route(&model_id, &auth_store)?;
        let (previous, due) = checkpoint_due(&active, previous, &meta, &config)?;
        if generation_mode == CheckpointGenerationMode::WhenDue && !due {
            return Ok(false);
        }
        let provider = create_provider(&provider_name)
            .ok_or_else(|| format!("Unknown compaction provider: {provider_name}"))?;
        let authoritative_state = self.authoritative_checkpoint_state();
        let handle = tokio::spawn(async move {
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
                authoritative_state,
                generation_mode,
            })
            .await
            .map_err(|error| error.to_string())
        });
        self.track_checkpoint_task(&model_id, purpose, handle);
        Ok(true)
    }

    fn compaction_session_path(&self) -> Result<&std::path::Path, String> {
        self.session
            .path()
            .ok_or_else(|| "compaction requires a durable session".to_string())
    }

    fn track_checkpoint_task(
        &mut self,
        model_id: &str,
        purpose: CheckpointPurpose,
        handle: tokio::task::JoinHandle<Result<(), String>>,
    ) {
        self.status_items.insert(
            "compaction-checkpoint".into(),
            format!("Checkpointing with {model_id}…"),
        );
        self.checkpoint_task = Some(CheckpointTask { handle, purpose });
    }

    pub(super) async fn collect_checkpoint_signal(&mut self) -> Option<super::RuntimeSignal> {
        let finished = self
            .checkpoint_task
            .as_ref()
            .is_some_and(|task| task.handle.is_finished());
        if !finished {
            return None;
        }
        let task = self.checkpoint_task.take()?;
        let purpose = task.purpose;
        Some(match task.handle.await {
            Ok(Ok(())) => super::RuntimeSignal::CheckpointTaskCompleted(purpose),
            Ok(Err(error)) => super::RuntimeSignal::CheckpointTaskFailed { purpose, error },
            Err(error) => super::RuntimeSignal::CheckpointTaskFailed {
                purpose,
                error: format!("Internal checkpoint task failure: {error}"),
            },
        })
    }

    pub(super) fn handle_checkpoint_completed(&mut self, purpose: CheckpointPurpose) {
        self.status_items.remove("compaction-checkpoint");
        if purpose == CheckpointPurpose::ManualCompaction {
            self.finish_manual_compaction(String::new());
        }
    }

    pub(super) fn handle_checkpoint_failed(&mut self, purpose: CheckpointPurpose, error: String) {
        self.status_items.remove("compaction-checkpoint");
        if purpose == CheckpointPurpose::ManualCompaction {
            self.push_error_msg(&format!(
                "Compaction checkpoint failed: {error}. Context was left unchanged."
            ));
        } else {
            self.push_warning_msg(&format!("Compaction checkpoint failed: {error}"));
        }
    }

    fn compaction_model_id<'a>(&'a self, configured: &'a str) -> &'a str {
        if configured.trim() == "default" {
            self.model_name.as_str()
        } else {
            configured.trim()
        }
    }

    fn resolve_checkpoint_route(
        &self,
        model_id: &str,
        auth_store: &AuthStore,
    ) -> Result<(ModelMeta, String), String> {
        let mut meta = self
            .model_registry
            .resolve_meta(model_id, None)
            .ok_or_else(|| format!("Unknown compaction model: {model_id}"))?;
        let mut provider_name = meta.provider.clone();
        if should_use_chatgpt_provider(auth_store, &self.model_registry, &meta) {
            provider_name = "openai-codex".to_string();
            meta = self
                .model_registry
                .resolve_meta(model_id, Some(&provider_name))
                .ok_or_else(|| {
                    format!("Compaction model {model_id} is unavailable through {provider_name}")
                })?;
        }
        Ok((meta, provider_name))
    }

    fn authoritative_checkpoint_state(&self) -> Option<String> {
        self.agent_task_state
            .as_ref()
            .and_then(|state| state.lock().ok())
            .filter(|state| state.should_project())
            .map(|state| state.projection())
    }
}

fn checkpoint_due(
    active: &[ActiveSessionMessage],
    mut previous: Option<CompactionCheckpoint>,
    meta: &ModelMeta,
    config: &imp_core::config::SummarizerConfig,
) -> Result<(Option<CompactionCheckpoint>, bool), String> {
    let source = match checkpoint_source_for_model(active, previous.as_ref(), meta) {
        Ok(source) => source,
        Err(_) => {
            previous = None;
            checkpoint_source_for_model(active, None, meta).map_err(|error| error.to_string())?
        }
    };
    Ok((previous, source.is_due(config.checkpoint_interval_tokens)))
}
