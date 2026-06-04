use super::*;
use imp_llm::model::ModelRegistry;

#[test]
fn selected_provider_and_step_clamp_stale_indices() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let mut state = WelcomeState::new(&models);
    state.step = usize::MAX;
    state.provider_selected = usize::MAX;
    state.web_provider_selected = usize::MAX;

    assert_eq!(state.current_step(), WelcomeStep::Done);
    assert!(state.selected_provider().is_some());
    assert!(state.selected_web_provider().is_some());
}

#[test]
fn empty_provider_lists_fail_gracefully() {
    let mut state = WelcomeState::new(&[]);
    state.providers.clear();
    state.web_providers.clear();

    assert!(state.selected_provider().is_none());
    assert!(state.selected_web_provider().is_none());
    assert!(state.check_auth_resolved().is_err());
    assert!(state.check_web_auth_resolved().is_err());
}

#[test]
fn setup_hides_kimi_code_provider_under_moonshot() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let state = WelcomeState::new(&models);

    assert!(state
        .providers
        .iter()
        .any(|provider| provider.meta.id == "moonshot"));
    assert!(!state
        .providers
        .iter()
        .any(|provider| provider.meta.id == "kimi-code"));
}

#[test]
fn openai_setup_models_include_gpt_5_5() {
    let registry = ModelRegistry::with_builtins();
    let models = filter_models_for_provider(registry.list(), "openai");

    let gpt_5_5 = models
        .iter()
        .find(|model| model.id == "gpt-5.5")
        .expect("OpenAI setup model list should include GPT-5.5");
    assert_eq!(gpt_5_5.provider, "openai");
}
