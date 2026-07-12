/// How a provider's API should be called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiStyle {
    /// Native Anthropic Messages API.
    Anthropic,
    /// Native OpenAI Responses API.
    OpenAi,
    /// ChatGPT/Codex-backed OpenAI Responses API.
    OpenAiCodex,
    /// Native Google Gemini API.
    Google,
    /// OpenAI-compatible Chat Completions API (DeepSeek, Groq, etc.).
    OpenAiCompat,
}

/// Metadata about an LLM provider.
#[derive(Debug, Clone)]
pub struct ProviderMeta {
    /// Provider identifier (e.g. "anthropic", "deepseek").
    pub id: &'static str,
    /// Human-readable name (e.g. "Anthropic", "DeepSeek").
    pub name: &'static str,
    /// Environment variable names for API key resolution, in priority order.
    pub env_vars: &'static [&'static str],
    /// Base URL for API requests. None for native providers that hardcode their URL.
    pub api_base_url: Option<&'static str>,
    /// URL where users can get an API key (shown in welcome flow).
    pub docs_url: &'static str,
    /// Which API protocol this provider uses.
    pub api_style: ApiStyle,
}

/// Registry of known LLM providers.
#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    providers: Vec<ProviderMeta>,
}

impl ProviderRegistry {
    /// Empty registry with no providers.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Registry pre-populated with all built-in providers.
    pub fn with_builtins() -> Self {
        Self {
            providers: builtin_providers(),
        }
    }

    /// Find a provider by its id (e.g. "anthropic", "deepseek").
    pub fn find(&self, id: &str) -> Option<&ProviderMeta> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// All registered providers.
    pub fn list(&self) -> &[ProviderMeta] {
        &self.providers
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Built-in provider catalogue covering all supported LLM providers.
pub fn builtin_providers() -> Vec<ProviderMeta> {
    vec![
        ProviderMeta {
            id: "anthropic",
            name: "Anthropic",
            env_vars: &["ANTHROPIC_API_KEY"],
            api_base_url: None,
            docs_url: "console.anthropic.com/settings/keys",
            api_style: ApiStyle::Anthropic,
        },
        ProviderMeta {
            id: "openai",
            name: "OpenAI",
            env_vars: &["OPENAI_API_KEY"],
            api_base_url: None,
            docs_url: "platform.openai.com/api-keys",
            api_style: ApiStyle::OpenAi,
        },
        ProviderMeta {
            id: "openai-codex",
            name: "ChatGPT",
            env_vars: &[],
            api_base_url: Some("https://chatgpt.com/backend-api"),
            docs_url: "chatgpt.com/codex",
            api_style: ApiStyle::OpenAiCodex,
        },
        ProviderMeta {
            id: "google",
            name: "Google",
            env_vars: &["GOOGLE_API_KEY"],
            api_base_url: None,
            docs_url: "aistudio.google.dev/apikey",
            api_style: ApiStyle::Google,
        },
        ProviderMeta {
            id: "deepseek",
            name: "DeepSeek",
            env_vars: &["DEEPSEEK_API_KEY"],
            api_base_url: Some("https://api.deepseek.com"),
            docs_url: "platform.deepseek.com/api_keys",
            api_style: ApiStyle::OpenAiCompat,
        },
        ProviderMeta {
            id: "moonshot",
            name: "Moonshot / Kimi",
            env_vars: &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
            api_base_url: Some("https://api.moonshot.ai"),
            docs_url: "platform.kimi.ai/console/api-keys",
            api_style: ApiStyle::OpenAiCompat,
        },
        ProviderMeta {
            id: "kimi-code",
            name: "Kimi Code",
            env_vars: &["KIMICODE_API_KEY"],
            api_base_url: Some("https://api.kimi.com/coding"),
            docs_url: "code.kimi.com",
            api_style: ApiStyle::OpenAiCompat,
        },
        ProviderMeta {
            id: "zai",
            name: "Z.AI",
            env_vars: &["ZAI_API_KEY"],
            api_base_url: Some("https://api.z.ai/api/paas/v4"),
            docs_url: "z.ai/model-api",
            api_style: ApiStyle::OpenAiCompat,
        },
        ProviderMeta {
            id: "openrouter",
            name: "OpenRouter",
            env_vars: &["OPENROUTER_API_KEY"],
            api_base_url: Some("https://openrouter.ai/api"),
            docs_url: "openrouter.ai/keys",
            api_style: ApiStyle::OpenAiCompat,
        },
        ProviderMeta {
            id: "groq",
            name: "Groq",
            env_vars: &["GROQ_API_KEY"],
            api_base_url: Some("https://api.groq.com/openai"),
            docs_url: "console.groq.com/keys",
            api_style: ApiStyle::OpenAiCompat,
        },
    ]
}
