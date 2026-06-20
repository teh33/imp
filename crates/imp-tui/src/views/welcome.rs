use imp_llm::auth::AuthStore;
use imp_llm::model::{ModelMeta, ProviderMeta, ProviderRegistry};
use imp_llm::ThinkingLevel;

use crate::app::WelcomeAuthMethod;
use crate::theme::Theme;

mod providers;
mod render;

use providers::{
    default_auth_method_for_provider, default_openrouter_model_meta, filter_models_for_provider,
    is_setup_visible_provider, provider_stored_for_setup, setup_provider_supports_api_key,
    setup_provider_supports_oauth,
};

/// Which step of the welcome flow the user is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeStep {
    /// Splash / introduction.
    Welcome,
    /// Choose provider and enter API key.
    ProviderAuth,
    /// Pick default model and thinking level.
    ModelThinking,
    /// Optional web search provider setup.
    WebSearch,
    /// Summary and quick tips.
    Done,
}

const STEPS: &[WelcomeStep] = &[
    WelcomeStep::Welcome,
    WelcomeStep::ModelThinking,
    WelcomeStep::Done,
];

/// Detected state for each provider — whether an env var or stored credential exists.
#[derive(Debug, Clone)]
pub struct ProviderStatus {
    pub meta: ProviderMeta,
    pub env_detected: bool,
    pub stored: bool,
}

impl ProviderStatus {
    pub fn has_auth(&self) -> bool {
        self.env_detected || self.stored
    }
}

#[derive(Debug, Clone)]
pub struct WebProviderStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub env_key: &'static str,
    pub docs_url: &'static str,
    pub env_detected: bool,
    pub stored: bool,
}

impl WebProviderStatus {
    pub fn has_auth(&self) -> bool {
        self.id == "none" || self.env_detected || self.stored
    }
}

/// State for the welcome overlay.
#[derive(Debug, Clone)]
pub struct WelcomeState {
    pub step: usize,
    /// Provider list with detection status.
    pub providers: Vec<ProviderStatus>,
    /// Currently selected provider index.
    pub provider_selected: usize,
    /// API key input buffer (masked display).
    pub key_input: String,
    /// Whether the key input field is active.
    pub key_editing: bool,
    /// Error message for invalid key input.
    pub key_error: Option<String>,
    /// Available models for the selected provider.
    pub models: Vec<ModelMeta>,
    /// Selected model index.
    pub model_selected: usize,
    /// Whether OpenRouter's default routing is selected.
    pub openrouter_default_model: bool,
    /// Selected thinking level.
    pub thinking_level: ThinkingLevel,
    /// Whether auth was resolved (env, OAuth, or input).
    pub auth_resolved: bool,
    /// Whether an OAuth login is in progress for the selected provider.
    pub oauth_pending: bool,
    /// Last OAuth URL shown when browser launch fails or as a fallback.
    pub oauth_url: Option<String>,
    /// Human-readable OAuth status for the selected provider.
    pub oauth_status: Option<String>,
    /// Selected auth method for providers that support both OAuth and API keys.
    pub auth_method: WelcomeAuthMethod,
    /// The resolved API key (if entered manually).
    pub resolved_key: Option<String>,
    /// Optional web search providers for the built-in `web` tool.
    pub web_providers: Vec<WebProviderStatus>,
    /// Selected web provider index.
    pub web_provider_selected: usize,
    /// Optional web provider key input.
    pub web_key_input: String,
    /// Resolved web provider id.
    pub resolved_web_provider: Option<String>,
    /// Resolved web provider key (if entered manually).
    pub resolved_web_key: Option<String>,
}

impl WelcomeState {
    fn normalized_step(&self) -> usize {
        self.step.min(STEPS.len().saturating_sub(1))
    }

    fn normalized_provider_selected(&self) -> usize {
        if self.providers.is_empty() {
            0
        } else {
            self.provider_selected.min(self.providers.len() - 1)
        }
    }

    fn normalized_model_selected(&self) -> usize {
        if self.models.is_empty() {
            0
        } else {
            self.model_selected.min(self.models.len() - 1)
        }
    }

    fn normalized_web_provider_selected(&self) -> usize {
        if self.web_providers.is_empty() {
            0
        } else {
            self.web_provider_selected.min(self.web_providers.len() - 1)
        }
    }

    /// Create welcome state, detecting existing auth from env vars for all registered providers.
    pub fn new(all_models: &[ModelMeta]) -> Self {
        let registry = ProviderRegistry::with_builtins();
        let auth_path = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
            .unwrap_or_else(|_| std::path::PathBuf::from(".config"))
            .join("imp")
            .join("auth.json");
        let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
        let providers: Vec<ProviderStatus> = registry
            .list()
            .iter()
            .filter(|meta| is_setup_visible_provider(meta.id))
            .map(|meta| {
                let env_detected = meta.env_vars.iter().any(|v| std::env::var(v).is_ok());
                ProviderStatus {
                    meta: meta.clone(),
                    env_detected,
                    stored: provider_stored_for_setup(&auth_store, meta.id),
                }
            })
            .collect();

        // Pre-select the first provider with auth, or the first provider (Anthropic) by default.
        let provider_selected = providers.iter().position(|p| p.has_auth()).unwrap_or(0);

        let selected_id = providers
            .get(provider_selected)
            .map(|provider| provider.meta.id)
            .unwrap_or("anthropic");
        let models = filter_models_for_provider(all_models, selected_id);

        let web_providers = vec![
            WebProviderStatus {
                id: "none",
                label: "Skip for now",
                env_key: "",
                docs_url: "",
                env_detected: false,
                stored: false,
            },
            WebProviderStatus {
                id: "tavily",
                label: "Tavily",
                env_key: "TAVILY_API_KEY",
                docs_url: "https://app.tavily.com/home",
                env_detected: std::env::var("TAVILY_API_KEY").is_ok(),
                stored: auth_store.stored.contains_key("tavily"),
            },
            WebProviderStatus {
                id: "exa",
                label: "Exa",
                env_key: "EXA_API_KEY",
                docs_url: "https://dashboard.exa.ai/api-keys",
                env_detected: std::env::var("EXA_API_KEY").is_ok(),
                stored: auth_store.stored.contains_key("exa"),
            },
        ];
        let web_provider_selected = web_providers.iter().position(|p| p.has_auth()).unwrap_or(0);

        Self {
            step: 0,
            providers,
            provider_selected,
            key_input: String::new(),
            key_editing: false,
            key_error: None,
            models,
            model_selected: 0,
            openrouter_default_model: true,
            thinking_level: ThinkingLevel::Medium,
            auth_resolved: false,
            oauth_pending: false,
            oauth_url: None,
            oauth_status: None,
            auth_method: default_auth_method_for_provider(selected_id),
            resolved_key: None,
            web_providers,
            web_provider_selected,
            web_key_input: String::new(),
            resolved_web_provider: None,
            resolved_web_key: None,
        }
    }

    /// Mark a provider as having a stored credential.
    pub fn mark_stored(&mut self, provider_id: &str) {
        for p in &mut self.providers {
            if p.meta.id == provider_id {
                p.stored = true;
            }
        }
    }

    pub fn current_step(&self) -> WelcomeStep {
        STEPS[self.normalized_step()]
    }

    pub fn selected_provider(&self) -> Option<&ProviderStatus> {
        self.providers.get(self.normalized_provider_selected())
    }

    /// Return the selected provider's id string.
    pub fn selected_provider_id(&self) -> Option<&str> {
        self.selected_provider().map(|provider| provider.meta.id)
    }

    pub fn selected_provider_supports_oauth(&self) -> bool {
        self.selected_provider_id()
            .is_some_and(setup_provider_supports_oauth)
    }

    pub fn selected_provider_supports_api_key_setup(&self) -> bool {
        self.selected_provider_id()
            .is_some_and(setup_provider_supports_api_key)
    }

    pub fn auth_method(&self) -> WelcomeAuthMethod {
        if !self.selected_provider_supports_oauth() {
            return WelcomeAuthMethod::ApiKey;
        }
        if !self.selected_provider_supports_api_key_setup() {
            return WelcomeAuthMethod::OAuth;
        }
        self.auth_method
    }

    pub fn toggle_auth_method(&mut self) {
        if self.selected_provider_supports_oauth()
            && self.selected_provider_supports_api_key_setup()
        {
            self.auth_method = match self.auth_method {
                WelcomeAuthMethod::OAuth => WelcomeAuthMethod::ApiKey,
                WelcomeAuthMethod::ApiKey => WelcomeAuthMethod::OAuth,
            };
            self.key_error = None;
        }
    }

    pub fn select_oauth_method(&mut self) {
        if self.selected_provider_supports_oauth() {
            self.auth_method = WelcomeAuthMethod::OAuth;
            self.key_error = None;
        }
    }

    pub fn select_api_key_method(&mut self) {
        if self.selected_provider_supports_api_key_setup() {
            self.auth_method = WelcomeAuthMethod::ApiKey;
            self.key_error = None;
        }
    }

    pub fn mark_oauth_pending(&mut self) {
        self.oauth_pending = true;
        self.oauth_url = None;
        self.oauth_status = Some("Starting OAuth login...".into());
        self.key_error = None;
    }

    pub fn set_oauth_url(&mut self, provider: &str, url: String, browser_opened: bool) {
        self.oauth_pending = true;
        self.oauth_url = Some(url);
        self.oauth_status = Some(if browser_opened {
            "Browser opened. Complete login there. If nothing opened, copy the URL below.".into()
        } else {
            format!(
                "Unable to open a browser here. Open this URL on your host machine, or run `imp login {provider}` in your shell."
            )
        });
    }

    /// Mark the selected provider as logged in after a successful OAuth flow.
    pub fn mark_selected_provider_oauth_complete(&mut self) {
        let Some(provider_id) = self.selected_provider_id().map(str::to_string) else {
            return;
        };
        self.mark_stored(&provider_id);
        self.oauth_pending = false;
        self.oauth_url = None;
        self.oauth_status = None;
        self.auth_resolved = true;
        self.resolved_key = None;
        self.key_input.clear();
        self.openrouter_default_model = true;
        self.key_error = None;
    }

    pub fn set_key_error(&mut self, error: impl Into<String>) {
        self.oauth_pending = false;
        self.oauth_status = Some(error.into());
        self.key_error = self.oauth_status.clone();
    }

    pub fn paste_key(&mut self, text: &str) {
        self.key_input.push_str(text.trim());
        self.key_error = None;
    }

    pub fn paste_web_key(&mut self, text: &str) {
        self.web_key_input.push_str(text.trim());
    }

    pub fn selected_model(&self) -> Option<ModelMeta> {
        if self.selected_provider_id() == Some("openrouter") {
            return Some(default_openrouter_model_meta());
        }
        self.models.get(self.normalized_model_selected()).cloned()
    }

    pub fn advance(&mut self) {
        if self.step + 1 < STEPS.len() {
            self.step += 1;
        }
    }

    pub fn go_back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }

    pub fn provider_up(&mut self) {
        if self.provider_selected > 0 {
            self.provider_selected -= 1;
            self.on_provider_changed();
        }
    }

    pub fn provider_down(&mut self) {
        if self.provider_selected + 1 < self.providers.len() {
            self.provider_selected += 1;
            self.on_provider_changed();
        }
    }

    pub fn model_up(&mut self) {
        if self.model_selected > 0 {
            self.model_selected -= 1;
        }
    }

    pub fn model_down(&mut self) {
        if self.model_selected + 1 < self.models.len() {
            self.model_selected += 1;
        }
    }

    pub fn cycle_thinking(&mut self) {
        self.thinking_level = match self.thinking_level {
            ThinkingLevel::Off => ThinkingLevel::Low,
            ThinkingLevel::Minimal => ThinkingLevel::Low,
            ThinkingLevel::Low => ThinkingLevel::Medium,
            ThinkingLevel::Medium => ThinkingLevel::High,
            ThinkingLevel::High => ThinkingLevel::XHigh,
            ThinkingLevel::XHigh => ThinkingLevel::Off,
        };
    }

    pub fn cycle_thinking_back(&mut self) {
        self.thinking_level = match self.thinking_level {
            ThinkingLevel::Off => ThinkingLevel::XHigh,
            ThinkingLevel::Minimal => ThinkingLevel::Off,
            ThinkingLevel::Low => ThinkingLevel::Off,
            ThinkingLevel::Medium => ThinkingLevel::Low,
            ThinkingLevel::High => ThinkingLevel::Medium,
            ThinkingLevel::XHigh => ThinkingLevel::High,
        };
    }

    pub fn push_key_char(&mut self, c: char) {
        self.key_input.push(c);
    }

    pub fn pop_key_char(&mut self) {
        self.key_input.pop();
    }

    /// Check whether auth is available for the current provider (env or entered key).
    pub fn check_auth_resolved(&mut self) -> Result<(), String> {
        let Some(status) = self.selected_provider() else {
            return Err("No providers available.".into());
        };
        if status.has_auth() {
            self.auth_resolved = true;
            self.resolved_key = None;
            return Ok(());
        }
        if !self.key_input.trim().is_empty() {
            self.auth_resolved = true;
            self.resolved_key = Some(self.key_input.trim().to_string());
            return Ok(());
        }
        Err("Please enter an API key or set the environment variable.".into())
    }

    pub fn update_models(&mut self, all_models: &[ModelMeta]) {
        let Some(id) = self.selected_provider_id().map(str::to_string) else {
            self.models.clear();
            self.model_selected = 0;
            return;
        };
        self.models = filter_models_for_provider(all_models, &id);
        self.model_selected = 0;
    }

    pub fn selected_web_provider(&self) -> Option<&WebProviderStatus> {
        self.web_providers
            .get(self.normalized_web_provider_selected())
    }

    pub fn web_provider_up(&mut self) {
        if self.web_provider_selected > 0 {
            self.web_provider_selected -= 1;
            self.on_web_provider_changed();
        }
    }

    pub fn web_provider_down(&mut self) {
        if self.web_provider_selected + 1 < self.web_providers.len() {
            self.web_provider_selected += 1;
            self.on_web_provider_changed();
        }
    }

    pub fn push_web_key_char(&mut self, c: char) {
        self.web_key_input.push(c);
    }

    pub fn pop_web_key_char(&mut self) {
        self.web_key_input.pop();
    }

    pub fn check_web_auth_resolved(&mut self) -> Result<(), String> {
        let (provider_id, has_auth) = {
            let Some(status) = self.selected_web_provider() else {
                return Err("No web search providers available.".into());
            };
            (status.id.to_string(), status.has_auth())
        };
        self.resolved_web_provider = Some(provider_id.clone());
        if provider_id == "none" {
            self.resolved_web_key = None;
            return Ok(());
        }
        if has_auth {
            self.resolved_web_key = None;
            return Ok(());
        }
        if !self.web_key_input.trim().is_empty() {
            self.resolved_web_key = Some(self.web_key_input.trim().to_string());
            return Ok(());
        }
        Err("Enter a web search API key or choose Skip for now.".into())
    }

    fn on_provider_changed(&mut self) {
        self.key_input.clear();
        self.key_editing = false;
        let new_provider_id = self
            .selected_provider_id()
            .unwrap_or("anthropic")
            .to_string();
        self.oauth_pending = false;
        self.oauth_url = None;
        self.oauth_status = None;
        self.auth_method = default_auth_method_for_provider(&new_provider_id);
        self.auth_resolved = false;
        self.resolved_key = None;
    }

    fn on_web_provider_changed(&mut self) {
        self.web_key_input.clear();
        self.resolved_web_key = None;
        self.resolved_web_provider = None;
    }
}

/// Detect whether this is a first run that needs the welcome flow.
///
/// Returns true when there is no user config AND no working auth for any
/// supported provider.
pub fn needs_welcome(config_dir: &std::path::Path, auth_path: &std::path::Path) -> bool {
    let config_exists = config_dir.join("config.toml").exists();
    if config_exists {
        return false;
    }

    // Check if any registered provider has auth via env var.
    let registry = ProviderRegistry::with_builtins();
    let has_env = registry
        .list()
        .iter()
        .any(|meta| meta.env_vars.iter().any(|v| std::env::var(v).is_ok()));

    let has_stored = auth_path.exists()
        && std::fs::read_to_string(auth_path)
            .map(|s| s.trim().len() > 2) // not empty JSON "{}"
            .unwrap_or(false);

    !has_env && !has_stored
}

// ── View widget ─────────────────────────────────────────────────

/// Welcome overlay widget.
pub struct WelcomeView<'a> {
    pub(super) state: &'a WelcomeState,
    pub(super) theme: &'a Theme,
}

impl<'a> WelcomeView<'a> {
    pub fn new(state: &'a WelcomeState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

#[cfg(test)]
#[path = "welcome/tests.rs"]
mod tests;
