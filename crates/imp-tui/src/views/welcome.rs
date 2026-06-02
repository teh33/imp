use imp_llm::auth::AuthStore;
use imp_llm::model::{ModelMeta, ProviderMeta, ProviderRegistry};
use imp_llm::ThinkingLevel;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::app::WelcomeAuthMethod;
use crate::theme::Theme;

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

fn is_setup_visible_provider(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "anthropic" | "openai" | "openrouter" | "moonshot"
    )
}

fn setup_provider_supports_oauth(provider_id: &str) -> bool {
    matches!(provider_id, "anthropic" | "openai")
}

fn default_auth_method_for_provider(provider_id: &str) -> WelcomeAuthMethod {
    if provider_id == "openai" {
        WelcomeAuthMethod::OAuth
    } else {
        WelcomeAuthMethod::ApiKey
    }
}

fn setup_provider_supports_api_key(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "anthropic" | "openai" | "openrouter" | "moonshot"
    )
}

fn default_openrouter_model_meta() -> ModelMeta {
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

fn provider_stored_for_setup(auth_store: &AuthStore, provider_id: &str) -> bool {
    auth_store.stored.contains_key(provider_id)
        || (provider_id == "moonshot" && auth_store.stored.contains_key("kimi-code"))
}

fn filter_models_for_provider(all_models: &[ModelMeta], provider_id: &str) -> Vec<ModelMeta> {
    let mut models: Vec<ModelMeta> = all_models
        .iter()
        .filter(|m| m.provider == provider_id)
        .cloned()
        .collect();

    match provider_id {
        "openai" => append_missing_openai_setup_models(&mut models),
        _ => {}
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
    state: &'a WelcomeState,
    theme: &'a Theme,
}

impl<'a> WelcomeView<'a> {
    pub fn new(state: &'a WelcomeState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for WelcomeView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 10 || area.width < 30 {
            return;
        }

        Clear.render(area, buf);

        let step_indicator = format!(
            " Welcome ({}/{}) ",
            self.state.normalized_step() + 1,
            STEPS.len()
        );
        let block = Block::default()
            .title(step_indicator)
            .borders(Borders::ALL)
            .border_style(self.theme.accent_style());
        let inner = block.inner(area);
        block.render(area, buf);

        match self.state.current_step() {
            WelcomeStep::Welcome => self.render_welcome(inner, buf),
            WelcomeStep::ProviderAuth => self.render_provider_auth(inner, buf, false),
            WelcomeStep::ModelThinking => self.render_model_thinking(inner, buf),
            WelcomeStep::WebSearch => self.render_web_search(inner, buf),
            WelcomeStep::Done => self.render_done(inner, buf),
        }
    }
}

impl WelcomeView<'_> {
    fn render_welcome(&self, area: Rect, buf: &mut Buffer) {
        self.render_provider_auth(area, buf, true);
    }

    fn render_provider_auth(&self, area: Rect, buf: &mut Buffer, include_intro: bool) {
        let mut row: u16 = 0;
        let x = area.x;

        let title_text = if include_intro {
            "  Welcome to imp — choose how to sign in"
        } else {
            "  Choose your AI provider"
        };
        let title = Line::from(Span::styled(
            title_text,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 2;

        for (i, status) in self.state.providers.iter().enumerate() {
            if row >= area.height.saturating_sub(4) {
                break;
            }
            let is_selected = i == self.state.provider_selected;
            let marker = if is_selected { "▸ " } else { "  " };

            let auth_hint = if status.env_detected {
                let detected_var = status
                    .meta
                    .env_vars
                    .iter()
                    .find(|v| std::env::var(v).is_ok())
                    .copied()
                    .unwrap_or(status.meta.env_vars.first().copied().unwrap_or(""));
                format!("  ({} detected ✓)", detected_var)
            } else if status.stored {
                "  (saved ✓)".to_string()
            } else {
                String::new()
            };

            let label_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let line = Line::from(vec![
                Span::styled(format!("  {marker}"), self.theme.accent_style()),
                Span::styled(status.meta.name, label_style),
                Span::styled(auth_hint, self.theme.success_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let Some(selected) = self.state.selected_provider() else {
            let line = Line::from(Span::styled(
                "  No providers available",
                self.theme.muted_style(),
            ));
            buf.set_line(x, area.y + row, &line, area.width);
            return;
        };
        if selected.has_auth() {
            let ready = Line::from(vec![
                Span::styled("  ✓ ", self.theme.success_style()),
                Span::styled("Ready to connect.", self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        } else {
            let oauth_supported = self.state.selected_provider_supports_oauth();
            let api_key_supported = self.state.selected_provider_supports_api_key_setup();
            if oauth_supported && api_key_supported {
                let oauth_style = if self.state.auth_method() == WelcomeAuthMethod::OAuth {
                    self.theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.muted_style()
                };
                let api_style = if self.state.auth_method() == WelcomeAuthMethod::ApiKey {
                    self.theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.muted_style()
                };
                let tabs = Line::from(vec![
                    Span::styled("  Auth: ", self.theme.muted_style()),
                    Span::styled("[ OAuth ]", oauth_style),
                    Span::raw("  "),
                    Span::styled("[ API key ]", api_style),
                    Span::styled("  Tab/←→ to switch", self.theme.muted_style()),
                ]);
                buf.set_line(x, area.y + row, &tabs, area.width);
                row += 2;
            }

            match self.state.auth_method() {
                WelcomeAuthMethod::OAuth if oauth_supported => {
                    if selected.meta.id == "anthropic" {
                        let warning = Line::from(Span::styled(
                            "  Anthropic OAuth is not supported. Proceed with caution.",
                            self.theme.error_style(),
                        ));
                        buf.set_line(x, area.y + row, &warning, area.width);
                        row += 1;
                    }

                    let oauth = if self.state.oauth_pending {
                        "  OAuth: waiting for browser login..."
                    } else {
                        "  OAuth: press Enter to sign in in your browser"
                    };
                    buf.set_line(
                        x,
                        area.y + row,
                        &Line::from(Span::styled(oauth, self.theme.accent_style())),
                        area.width,
                    );
                    row += 1;

                    if let Some(status) = self.state.oauth_status.as_deref() {
                        let status_line = Line::from(Span::styled(
                            format!("  {status}"),
                            self.theme.muted_style(),
                        ));
                        buf.set_line(x, area.y + row, &status_line, area.width);
                        row += 1;
                    }
                    if let Some(url) = self.state.oauth_url.as_deref() {
                        let url_line = Line::from(vec![
                            Span::styled("  URL: ", self.theme.muted_style()),
                            Span::styled(url, Style::default().fg(self.theme.accent)),
                        ]);
                        buf.set_line(x, area.y + row, &url_line, area.width);
                        row += 1;
                    }
                }
                _ if api_key_supported => {
                    let prompt_line =
                        Line::from(vec![Span::styled("  API Key: ", self.theme.muted_style())]);
                    buf.set_line(x, area.y + row, &prompt_line, area.width);
                    row += 1;

                    let display_key = if self.state.key_input.is_empty() {
                        "  ┌─ paste your key here ─────────────────┐".to_string()
                    } else {
                        let masked: String = self
                            .state
                            .key_input
                            .chars()
                            .enumerate()
                            .map(|(i, c)| if i < 6 { c } else { '•' })
                            .collect();
                        format!(
                            "  ┌ {masked}▎{} ┐",
                            " ".repeat(40usize.saturating_sub(masked.len() + 1))
                        )
                    };
                    let key_style = if self.state.key_input.is_empty() {
                        self.theme.muted_style()
                    } else {
                        Style::default()
                    };
                    let key_line = Line::from(Span::styled(display_key, key_style));
                    buf.set_line(x, area.y + row, &key_line, area.width);
                    row += 1;

                    let url_line = Line::from(vec![
                        Span::styled("  Get a key: ", self.theme.muted_style()),
                        Span::styled(
                            selected.meta.docs_url,
                            Style::default().fg(self.theme.accent),
                        ),
                    ]);
                    buf.set_line(x, area.y + row, &url_line, area.width);
                    row += 1;
                }
                _ => {}
            }

            if let Some(ref error) = self.state.key_error {
                row += 1;
                let error_line =
                    Line::from(Span::styled(format!("  {error}"), self.theme.error_style()));
                buf.set_line(x, area.y + row, &error_line, area.width);
            }
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::styled("    ", Style::default()),
                Span::styled("O ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("OAuth login", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Select provider", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_model_thinking(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let title = Line::from(Span::styled(
            "  Default model & thinking level",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 2;

        let subtitle = Line::from(Span::styled("  Model:", self.theme.muted_style()));
        buf.set_line(x, area.y + row, &subtitle, area.width);
        row += 1;

        let visible_models = 6usize;
        let selected_model = self.state.normalized_model_selected();
        let start = selected_model.saturating_sub(visible_models / 2);
        let end = (start + visible_models).min(self.state.models.len());
        let start = end.saturating_sub(visible_models);

        for model_i in start..end {
            if row >= area.height.saturating_sub(6) {
                break;
            }
            let model = &self.state.models[model_i];
            let is_selected = model_i == selected_model;
            let marker = if is_selected { "▸ " } else { "  " };

            let name_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let context_str = format!("{}k", model.context_window / 1000);
            let price_str = format!(
                "${:.2}/{:.2}",
                model.pricing.input_per_mtok, model.pricing.output_per_mtok
            );

            let line = Line::from(vec![
                Span::styled(format!("    {marker}"), self.theme.accent_style()),
                Span::styled(format!("{:<36}", &model.name), name_style),
                Span::styled(format!("{context_str:>5}"), self.theme.muted_style()),
                Span::raw("  "),
                Span::styled(price_str, self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let thinking_label = match self.state.thinking_level {
            ThinkingLevel::Off => "Off",
            ThinkingLevel::Minimal => "Minimal",
            ThinkingLevel::Low => "Low",
            ThinkingLevel::Medium => "Medium",
            ThinkingLevel::High => "High",
            ThinkingLevel::XHigh => "XHigh",
        };
        let thinking_line = Line::from(vec![
            Span::styled("  Thinking:  ", self.theme.muted_style()),
            Span::styled("← ", self.theme.accent_style()),
            Span::styled(
                thinking_label,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" →", self.theme.accent_style()),
        ]);
        buf.set_line(x, area.y + row, &thinking_line, area.width);
        row += 2;

        let hint = Line::from(Span::styled(
            "  You can change these anytime with Ctrl+L and Shift+Tab.",
            self.theme.muted_style(),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &hint, area.width);
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Model", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("←→ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Thinking", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_web_search(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let title = Line::from(Span::styled(
            "  Optional web search setup",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 1;

        let subtitle = Line::from(Span::styled(
            "  Add Tavily or Exa now so the web tool can search immediately.",
            self.theme.muted_style(),
        ));
        buf.set_line(x, area.y + row, &subtitle, area.width);
        row += 2;

        for (i, provider) in self.state.web_providers.iter().enumerate() {
            if row >= area.height.saturating_sub(6) {
                break;
            }
            let is_selected = i == self.state.web_provider_selected;
            let marker = if is_selected { "▸ " } else { "  " };
            let mut status = String::new();
            if provider.id == "none" {
                status = "  (skip)".to_string();
            } else if provider.env_detected {
                status = format!("  ({} detected ✓)", provider.env_key);
            } else if provider.stored {
                status = "  (saved ✓)".to_string();
            }
            let label_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let line = Line::from(vec![
                Span::styled(format!("  {marker}"), self.theme.accent_style()),
                Span::styled(provider.label, label_style),
                Span::styled(status, self.theme.success_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;
        let Some(selected) = self.state.selected_web_provider() else {
            let line = Line::from(Span::styled(
                "  No web search providers available",
                self.theme.muted_style(),
            ));
            buf.set_line(x, area.y + row, &line, area.width);
            return;
        };
        if selected.id != "none" && !selected.has_auth() {
            let prompt_line =
                Line::from(vec![Span::styled("  API Key: ", self.theme.muted_style())]);
            buf.set_line(x, area.y + row, &prompt_line, area.width);
            row += 1;

            let display_key = if self.state.web_key_input.is_empty() {
                "  ┌─ paste your key here ─────────────────┐".to_string()
            } else {
                let masked: String = self
                    .state
                    .web_key_input
                    .chars()
                    .enumerate()
                    .map(|(i, c)| if i < 6 { c } else { '•' })
                    .collect();
                format!(
                    "  ┌ {masked}▎{} ┐",
                    " ".repeat(40usize.saturating_sub(masked.len() + 1))
                )
            };
            let key_style = if self.state.web_key_input.is_empty() {
                self.theme.muted_style()
            } else {
                Style::default()
            };
            let key_line = Line::from(Span::styled(display_key, key_style));
            buf.set_line(x, area.y + row, &key_line, area.width);
            row += 1;

            let url_line = Line::from(vec![
                Span::styled("  Get a key: ", self.theme.muted_style()),
                Span::styled(selected.docs_url, Style::default().fg(self.theme.accent)),
            ]);
            buf.set_line(x, area.y + row, &url_line, area.width);
        } else if selected.id == "none" {
            let ready = Line::from(vec![
                Span::styled("  ↷ ", self.theme.muted_style()),
                Span::styled(
                    "Skipping web search setup for now.",
                    self.theme.muted_style(),
                ),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        } else {
            let ready = Line::from(vec![
                Span::styled("  ✓ ", self.theme.success_style()),
                Span::styled("Web search provider is ready.", self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Select provider", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_done(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let header = Line::from(Span::styled(
            "  ✓ You're all set.",
            Style::default()
                .fg(self.theme.success)
                .add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &header, area.width);
        row += 2;

        let provider_name = self
            .state
            .selected_provider()
            .map(|provider| provider.meta.name)
            .unwrap_or("not configured");
        let web_provider_name = self
            .state
            .resolved_web_provider
            .as_deref()
            .filter(|id| *id != "none")
            .map(|id| {
                self.state
                    .web_providers
                    .iter()
                    .find(|provider| provider.id == id)
                    .map(|provider| provider.label)
                    .unwrap_or(id)
            })
            .unwrap_or("not configured");
        let model_name = self
            .state
            .selected_model()
            .map(|m| m.name)
            .unwrap_or_else(|| "default".to_string());
        let thinking_label = match self.state.thinking_level {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
        };

        let summary_lines = [
            format!("  Provider:  {provider_name}"),
            format!("  Model:     {model_name}"),
            format!("  Thinking:  {thinking_label}"),
            format!("  Web:       {web_provider_name}"),
        ];

        for line_text in &summary_lines {
            if row >= area.height {
                return;
            }
            let line = Line::from(Span::styled(line_text.as_str(), Style::default()));
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let config_hint = Line::from(Span::styled(
            "  Config saved to ~/.config/imp/config.toml",
            self.theme.muted_style(),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &config_hint, area.width);
            row += 1;
        }

        row += 1;

        let tips_header = Line::from(Span::styled(
            "  Quick tips:",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &tips_header, area.width);
            row += 1;
        }

        let tips = [
            ("Enter", "Send a message"),
            ("Ctrl+C", "Clear / Abort / Quit"),
            ("Ctrl+L", "Switch model"),
            ("Shift+Tab", "Cycle thinking level"),
            ("@file", "Attach file context"),
            ("/command", "Slash commands"),
        ];

        for (key, desc) in &tips {
            if row >= area.height.saturating_sub(2) {
                break;
            }
            let line = Line::from(vec![
                Span::styled(format!("    {key:<12}"), self.theme.accent_style()),
                Span::styled(*desc, self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Start using imp", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
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
}
