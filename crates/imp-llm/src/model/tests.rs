use super::*;

#[test]
fn find_by_alias_resolves_sonnet() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("sonnet")
        .expect("sonnet alias should resolve");
    assert_eq!(model.id, "claude-sonnet-4-6");
    assert_eq!(model.provider, "anthropic");
}

#[test]
fn find_by_alias_resolves_haiku() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("haiku")
        .expect("haiku alias should resolve");
    assert_eq!(model.id, "claude-haiku-4-5-20251001");
}

#[test]
fn find_by_alias_resolves_opus() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("opus")
        .expect("opus alias should resolve");
    assert_eq!(model.id, "claude-opus-4-6");
}

#[test]
fn find_by_alias_resolves_gpt5() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("gpt5")
        .expect("gpt5 alias should resolve");
    assert_eq!(model.id, "gpt-5.4");
}

#[test]
fn resolve_meta_synthesizes_gpt_5_6_alias() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("gpt5.6", None)
        .expect("gpt5.6 alias should resolve");
    assert_eq!(model.id, "gpt-5.6");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.context_window, 1_050_000);
}

#[test]
fn resolve_meta_synthesizes_gpt_5_5_alias() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("gpt5.5", None)
        .expect("gpt5.5 alias should synthesize");
    assert_eq!(model.id, "gpt-5.5");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.context_window, 1_050_000);
}

#[test]
fn resolve_meta_respects_chatgpt_gpt_5_5_window_with_codex_hint() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("gpt5.5", Some("openai-codex"))
        .expect("gpt5.5 alias should synthesize for ChatGPT/Codex");
    assert_eq!(model.id, "gpt-5.5");
    assert_eq!(model.provider, "openai-codex");
    assert_eq!(model.context_window, 1_050_000);
}

#[test]
fn find_by_alias_resolves_chatgpt() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("chatgpt")
        .expect("chatgpt alias should resolve");
    assert_eq!(model.id, "gpt-5.3-chat-latest");
}

#[test]
fn find_by_alias_resolves_codex() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("codex")
        .expect("codex alias should resolve");
    assert_eq!(model.id, "gpt-5.3-codex");
}

#[test]
fn resolve_meta_synthesizes_spark_preview() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("spark", None)
        .expect("spark alias should synthesize");
    assert_eq!(model.id, "gpt-5.3-codex-spark");
    assert_eq!(model.provider, "openai");
}

#[test]
fn resolve_meta_synthesizes_legacy_openai_model() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("gpt-4o", None)
        .expect("legacy openai model should synthesize");
    assert_eq!(model.id, "gpt-4o");
    assert_eq!(model.provider, "openai");
}

#[test]
fn find_by_alias_resolves_gemini_pro() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("gemini-pro")
        .expect("gemini-pro alias should resolve");
    assert_eq!(model.id, "gemini-2.5-pro");
}

#[test]
fn find_by_alias_resolves_kimi() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("kimi")
        .expect("kimi alias should resolve");
    assert_eq!(model.id, "kimi-k2.6");
    assert_eq!(model.provider, "moonshot");
}

#[test]
fn find_by_alias_resolves_kimi_turbo() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("kimi-k2-turbo")
        .expect("kimi-k2-turbo alias should resolve");
    assert_eq!(model.id, "kimi-k2-turbo-preview");
    assert_eq!(model.provider, "moonshot");
}

#[test]
fn resolve_meta_guesses_moonshot_for_kimi_models() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("kimi-k2-thinking-turbo", None)
        .expect("kimi model should synthesize");
    assert_eq!(model.id, "kimi-k2-thinking-turbo");
    assert_eq!(model.provider, "moonshot");
}

#[test]
fn find_by_alias_resolves_zai_glm() {
    let reg = ModelRegistry::with_builtins();
    let model = reg.find_by_alias("zai").expect("zai alias should resolve");
    assert_eq!(model.id, "glm-4.7");
    assert_eq!(model.provider, "zai");
}

#[test]
fn resolve_meta_guesses_zai_for_glm_models() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .resolve_meta("glm-5-air", None)
        .expect("glm model should synthesize");
    assert_eq!(model.id, "glm-5-air");
    assert_eq!(model.provider, "zai");
    assert!(model.capabilities.reasoning);
}

#[test]
fn provider_registry_includes_zai() {
    let registry = ProviderRegistry::with_builtins();
    let provider = registry.find("zai").expect("zai provider should exist");
    assert_eq!(provider.name, "Z.AI");
    assert_eq!(provider.api_base_url, Some("https://api.z.ai/api/paas/v4"));
    assert_eq!(provider.env_vars, &["ZAI_API_KEY"]);
}

#[test]
fn provider_registry_includes_moonshot() {
    let registry = ProviderRegistry::with_builtins();
    let provider = registry
        .find("moonshot")
        .expect("moonshot provider should exist");
    assert_eq!(provider.name, "Moonshot / Kimi");
    assert_eq!(provider.api_base_url, Some("https://api.moonshot.ai"));
    assert_eq!(provider.env_vars, &["MOONSHOT_API_KEY", "KIMI_API_KEY"]);
}

#[test]
fn find_by_alias_falls_back_to_exact_id() {
    let reg = ModelRegistry::with_builtins();
    let model = reg
        .find_by_alias("gpt-5.3-codex")
        .expect("exact id lookup should work as fallback");
    assert_eq!(model.id, "gpt-5.3-codex");
}

#[test]
fn find_by_alias_returns_none_for_unknown() {
    let reg = ModelRegistry::with_builtins();
    assert!(reg.find_by_alias("nonexistent-model").is_none());
}

#[test]
fn list_by_provider_filters_correctly() {
    let reg = ModelRegistry::with_builtins();
    let anthropic = reg.list_by_provider("anthropic");
    assert_eq!(anthropic.len(), 3);
    assert!(anthropic.iter().all(|m| m.provider == "anthropic"));

    let openai = reg.list_by_provider("openai");
    assert_eq!(openai.len(), 6);

    let google = reg.list_by_provider("google");
    assert_eq!(google.len(), 2);

    let moonshot = reg.list_by_provider("moonshot");
    assert_eq!(moonshot.len(), 6);
}

#[test]
fn builtin_openai_codex_models_retag_openai_models() {
    let models = builtin_openai_codex_models();
    assert_eq!(models.len(), 7);
    assert!(models.iter().all(|model| model.provider == "openai-codex"));
    let gpt_5_5 = models
        .iter()
        .find(|model| model.id == "gpt-5.5")
        .expect("OpenAI Codex model list should include GPT-5.5");
    assert_eq!(gpt_5_5.context_window, 1_050_000);
}

#[test]
fn register_skips_duplicates() {
    let mut reg = ModelRegistry::new();
    let meta = ModelMeta {
        id: "test-model".into(),
        provider: "test".into(),
        name: "Test".into(),
        context_window: 1000,
        max_output_tokens: 100,
        pricing: ModelPricing::default(),
        capabilities: Capabilities::default(),
    };
    reg.register(meta.clone());
    reg.register(meta);
    assert_eq!(reg.list().len(), 1);
}
