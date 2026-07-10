use imp_core::config::Config;
use imp_llm::auth::AuthStore;
use imp_llm::model::{ModelMeta, ModelRegistry};

pub(super) fn model_supports_provider(
    registry: &ModelRegistry,
    provider: &str,
    model_id: &str,
) -> bool {
    if provider == "openai-codex" {
        return imp_llm::model::builtin_openai_codex_models()
            .iter()
            .any(|model| model.id == model_id);
    }

    registry
        .list_by_provider(provider)
        .iter()
        .any(|model| model.id == model_id)
}

pub(super) fn should_use_chatgpt_provider(
    auth_store: &AuthStore,
    registry: &ModelRegistry,
    meta: &ModelMeta,
) -> bool {
    meta.provider == "openai"
        && auth_store.resolve_api_key_only("openai").is_err()
        && (auth_store.get_oauth("openai").is_some()
            || auth_store.get_oauth("openai-codex").is_some())
        && model_supports_provider(registry, "openai-codex", &meta.id)
}

pub(super) async fn resolve_provider_api_key(
    auth_store: &mut AuthStore,
    provider_name: &str,
) -> Result<String, imp_llm::Error> {
    match provider_name {
        "openai" => auth_store.resolve_api_key_only(provider_name),
        "openai-codex" => auth_store.resolve_chatgpt_oauth().await,
        _ => auth_store.resolve_with_refresh(provider_name).await,
    }
}

pub(super) fn provider_logged_in(auth_store: &AuthStore, provider: &str) -> bool {
    match provider {
        "openai" => {
            auth_store.get_oauth("openai").is_some()
                || auth_store.get_oauth("openai-codex").is_some()
                || auth_store.has_credentials("openai")
        }
        _ => auth_store.has_credentials(provider),
    }
}

pub(super) fn oauth_provider(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic" | "openai" | "openai-codex" | "kimi-code"
    )
}

pub(super) fn model_picker_chatgpt_oauth_models(
    _registry: &ModelRegistry,
    auth_store: &AuthStore,
) -> Vec<ModelMeta> {
    let has_chatgpt_oauth =
        auth_store.get_oauth("openai").is_some() || auth_store.get_oauth("openai-codex").is_some();
    if !has_chatgpt_oauth || auth_store.resolve_api_key_only("openai").is_ok() {
        return Vec::new();
    }

    imp_llm::model::builtin_openai_codex_models()
        .into_iter()
        .map(|mut model| {
            model.provider = "openai".into();
            model
        })
        .collect()
}

pub(super) fn merge_model_options_with_oauth_only_models(
    mut models: Vec<ModelMeta>,
    oauth_only_models: Vec<ModelMeta>,
) -> Vec<ModelMeta> {
    if oauth_only_models.is_empty() {
        return models;
    }

    let insert_at = models
        .iter()
        .rposition(|model| model.provider == "openai")
        .map_or(models.len(), |index| index + 1);
    models.splice(insert_at..insert_at, oauth_only_models);
    models
}

pub(super) fn filtered_model_options(
    registry: &ModelRegistry,
    config: &Config,
    auth_store: &AuthStore,
) -> Vec<ModelMeta> {
    let oauth_only_models = model_picker_chatgpt_oauth_models(registry, auth_store);

    match &config.enabled_models {
        Some(enabled) if !enabled.is_empty() => {
            let available_models = merge_model_options_with_oauth_only_models(
                registry.list().to_vec(),
                oauth_only_models,
            );

            let available_ids: std::collections::HashSet<&str> =
                available_models.iter().map(|m| m.id.as_str()).collect();
            let enabled_ids: std::collections::HashSet<String> = enabled
                .iter()
                .filter_map(|name| registry.resolve_meta(name, None).map(|model| model.id))
                .filter(|id| available_ids.contains(id.as_str()))
                .collect();

            available_models
                .into_iter()
                .filter(|model| enabled_ids.contains(&model.id))
                .collect()
        }
        _ => {
            let visible_models: Vec<ModelMeta> = registry
                .list()
                .iter()
                .filter(|model| auth_store.has_credentials(&model.provider))
                .cloned()
                .collect();
            merge_model_options_with_oauth_only_models(visible_models, oauth_only_models)
        }
    }
}

pub(super) fn include_current_model_option(
    mut models: Vec<ModelMeta>,
    registry: &ModelRegistry,
    current_model: &str,
) -> (Vec<ModelMeta>, String) {
    let Some(meta) = registry.resolve_meta(current_model, None) else {
        return (models, current_model.to_string());
    };

    let canonical_id = meta.id.clone();
    if !models.iter().any(|model| model.id == canonical_id) {
        models.insert(0, meta);
    }

    (models, canonical_id)
}
