use imp_llm::auth::AuthStore;
use imp_llm::model::ModelMeta;

use crate::app::WelcomeAuthMethod;

pub(super) fn is_setup_visible_provider(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "anthropic" | "openai" | "openrouter" | "moonshot"
    )
}

pub(super) fn setup_provider_supports_oauth(provider_id: &str) -> bool {
    matches!(provider_id, "anthropic" | "openai")
}

pub(super) fn default_auth_method_for_provider(provider_id: &str) -> WelcomeAuthMethod {
    if provider_id == "openai" {
        WelcomeAuthMethod::OAuth
    } else {
        WelcomeAuthMethod::ApiKey
    }
}

pub(super) fn setup_provider_supports_api_key(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "anthropic" | "openai" | "openrouter" | "moonshot"
    )
}

pub(super) fn default_openrouter_model_meta() -> ModelMeta {
    ModelMeta {
        id: "openrouter/auto".to_string(),
        provider: "openrouter".into(),
        name: "OpenRouter Default".into(),
        context_window: 200_000,
        max_output_tokens: 16_384,
        pricing: Default::default(),
        capabilities: imp_llm::model::Capabilities {
            reasoning: true,
            images: true,
            tool_use: true,
        },
    }
}

pub(super) fn provider_stored_for_setup(auth_store: &AuthStore, provider_id: &str) -> bool {
    auth_store.stored.contains_key(provider_id)
        || (provider_id == "moonshot" && auth_store.stored.contains_key("kimi-code"))
}

pub(super) fn filter_models_for_provider(
    all_models: &[ModelMeta],
    provider_id: &str,
) -> Vec<ModelMeta> {
    let mut models: Vec<ModelMeta> = all_models
        .iter()
        .filter(|m| m.provider == provider_id)
        .cloned()
        .collect();

    if provider_id == "openai" {
        append_missing_openai_setup_models(&mut models);
    }

    models
}

fn append_missing_openai_setup_models(models: &mut Vec<ModelMeta>) {
    for mut model in imp_llm::model::builtin_openai_codex_models() {
        if models.iter().any(|existing| existing.id == model.id) {
            continue;
        }
        model.provider = "openai".into();
        models.push(model);
    }
}
