use super::*;
use imp_core::config::Config;
use imp_llm::auth::AuthStore;
use imp_llm::model::ModelRegistry;

#[test]
fn applying_settings_forces_primary_inspector_display_model() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut config = Config::default();
    let state = SettingsState::new(&config, &models[0].id, &models, &auth_store);

    state.apply_to_config(&mut config);

    assert_eq!(config.ui.sidebar_style, SidebarStyle::Inspector);
    assert_eq!(config.ui.tool_output, ToolOutputDisplay::Full);
    assert_eq!(config.ui.chat_tool_display, ChatToolDisplay::Summary);
    assert!(!config.ui.hide_tools_in_chat);
}

#[test]
fn save_field_scrolls_into_view_on_short_panels() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let config = Config::default();
    let mut state = SettingsState::new(&config, &models[0].id, &models, &auth_store);
    state.tab = SettingsTab::Ui;
    state.selected = field_index(SettingsField::Save);

    assert_eq!(selected_settings_row(&state), 18);
    assert_eq!(total_settings_rows(&state), 19);
    assert_eq!(settings_scroll_offset(&state, 10), 9);
}

#[test]
fn custom_theme_value_is_selectable_and_cycles() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let config = Config {
        theme: Some("custom-highlighter".into()),
        ..Config::default()
    };
    let mut state = SettingsState::new(&config, &models[0].id, &models, &auth_store);

    assert_eq!(state.theme_name, "custom-highlighter");
    assert!(state
        .theme_options
        .iter()
        .any(|theme| theme == "custom-highlighter"));

    state.selected = field_index(SettingsField::Theme);
    state.cycle_forward();
    assert_eq!(state.theme_name, "default");
    state.cycle_backward();
    assert_eq!(state.theme_name, "custom-highlighter");
}

#[test]
fn top_fields_do_not_scroll_when_visible() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let config = Config::default();
    let mut state = SettingsState::new(&config, &models[0].id, &models, &auth_store);

    assert_eq!(selected_settings_row(&state), 4);
    assert_eq!(settings_scroll_offset(&state, 10), 0);

    state.move_down();
    assert_eq!(selected_settings_row(&state), 5);
    assert_eq!(settings_scroll_offset(&state, 10), 0);
}

#[test]
fn current_field_clamps_stale_selection() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let state = SettingsState {
        selected: usize::MAX,
        ..SettingsState::new(&Config::default(), &models[0].id, &models, &auth_store)
    };

    assert_eq!(state.current_field(), SettingsField::Save);
}

#[test]
fn cycle_model_is_safe_with_empty_model_options() {
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut state = SettingsState::new(&Config::default(), "custom-model", &[], &auth_store);
    state.selected = 0;
    state.model_options.clear();

    state.cycle_forward();
    state.cycle_backward();

    assert_eq!(state.model, "custom-model");
}

#[test]
fn chosen_models_round_trip_into_config() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut config = Config::default();
    let mut state = SettingsState::new(&config, &models[0].id, &models, &auth_store);

    state.tab = SettingsTab::Model;
    state.selected = field_index(SettingsField::ChosenModels);
    state.cycle_forward();
    assert_eq!(state.chosen_models, vec![models[0].id.clone()]);

    state.apply_to_config(&mut config);
    assert_eq!(config.enabled_models, Some(vec![models[0].id.clone()]));
}

#[test]
fn bell_setting_round_trips_into_config() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut config = Config::default();
    let state = SettingsState {
        notify_on_agent_complete: false,
        ..SettingsState::new(&config, &models[0].id, &models, &auth_store)
    };

    state.apply_to_config(&mut config);
    assert!(!config.ui.notify_on_agent_complete);
}

#[test]
fn continue_policy_round_trips_into_config() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut config = Config::default();
    let state = SettingsState {
        continue_policy: ContinuePolicy::Balanced,
        ..SettingsState::new(&config, &models[0].id, &models, &auth_store)
    };

    state.apply_to_config(&mut config);
    assert_eq!(config.ui.continue_policy, ContinuePolicy::Balanced);
}

#[test]
fn empty_chosen_models_means_all_models() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list().to_vec();
    let auth_store = AuthStore::new(std::path::PathBuf::from("/tmp/auth.json"));
    let mut config = Config::default();
    let state = SettingsState::new(&config, &models[0].id, &models, &auth_store);

    state.apply_to_config(&mut config);
    assert_eq!(config.enabled_models, None);
}
