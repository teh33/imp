use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

mod providers;

pub use providers::{builtin_providers, ApiStyle, ProviderMeta, ProviderRegistry};

/// Static metadata describing a model's capabilities and pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    /// Canonical model identifier (e.g. "claude-sonnet-4-6").
    pub id: String,
    /// Provider that serves this model (e.g. "anthropic").
    pub provider: String,
    /// Human-readable display name.
    pub name: String,
    /// Maximum input context in tokens.
    pub context_window: u32,
    /// Maximum tokens the model can generate.
    pub max_output_tokens: u32,
    /// Per-million-token pricing.
    pub pricing: ModelPricing,
    /// Feature flags.
    pub capabilities: Capabilities,
}

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Dollars per million input tokens.
    pub input_per_mtok: f64,
    /// Dollars per million output tokens.
    pub output_per_mtok: f64,
    /// Dollars per million cache-read tokens.
    pub cache_read_per_mtok: f64,
    /// Dollars per million cache-write tokens.
    pub cache_write_per_mtok: f64,
}

/// Feature flags indicating what a model supports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Supports extended thinking / chain-of-thought.
    pub reasoning: bool,
    /// Supports image inputs.
    pub images: bool,
    /// Supports tool/function calling.
    pub tool_use: bool,
}

/// Resolved model ready for use (metadata + provider reference).
pub struct Model {
    /// Static metadata for this model.
    pub meta: ModelMeta,
    /// The provider that will serve requests.
    pub provider: Arc<dyn Provider>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model")
            .field("meta", &self.meta)
            .field("provider", &self.provider.id())
            .finish()
    }
}

/// Central index of available models with alias resolution.
///
/// Stores [`ModelMeta`] entries and short aliases (e.g. "sonnet" → canonical id).
/// Create with [`ModelRegistry::with_builtins`] for a pre-populated registry.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    models: Vec<ModelMeta>,
    aliases: HashMap<String, String>,
}

impl ModelRegistry {
    /// Empty registry with no models or aliases.
    pub fn new() -> Self {
        Self {
            models: Vec::new(),
            aliases: HashMap::new(),
        }
    }

    /// Registry pre-populated with built-in models and aliases for
    /// Anthropic, OpenAI, and Google.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        for meta in builtin_models() {
            reg.register(meta);
        }
        for (alias, canonical) in builtin_aliases() {
            reg.aliases.insert(alias, canonical);
        }
        reg
    }

    /// Add a model to the registry.
    pub fn register(&mut self, meta: ModelMeta) {
        // Avoid duplicates by id.
        if !self.models.iter().any(|m| m.id == meta.id) {
            self.models.push(meta);
        }
    }

    /// Register a short alias that maps to a canonical model id.
    pub fn register_alias(&mut self, alias: impl Into<String>, canonical_id: impl Into<String>) {
        self.aliases.insert(alias.into(), canonical_id.into());
    }

    /// Find a model by exact canonical id.
    pub fn find(&self, id: &str) -> Option<&ModelMeta> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Resolve an alias to a model. Falls back to exact-id lookup if no alias matches.
    pub fn find_by_alias(&self, alias: &str) -> Option<&ModelMeta> {
        if let Some(canonical) = self.aliases.get(alias) {
            self.find(canonical)
        } else {
            self.find(alias)
        }
    }

    /// All registered models.
    pub fn list(&self) -> &[ModelMeta] {
        &self.models
    }

    /// Models from a specific provider.
    pub fn list_by_provider(&self, provider: &str) -> Vec<&ModelMeta> {
        self.models
            .iter()
            .filter(|m| m.provider == provider)
            .collect()
    }

    /// Resolve a built-in model, or synthesize metadata for a custom model id.
    pub fn resolve_meta(&self, model_name: &str, provider_hint: Option<&str>) -> Option<ModelMeta> {
        let canonical_name = self
            .aliases
            .get(model_name)
            .map(String::as_str)
            .unwrap_or(model_name);
        let validated_provider_hint = provider_hint
            .filter(|provider| ProviderRegistry::with_builtins().find(provider).is_some());

        if let Some(meta) = self.find(canonical_name) {
            if let Some(provider_hint) = validated_provider_hint {
                if provider_hint != meta.provider {
                    return Some(synthesize_custom_model_meta(canonical_name, provider_hint));
                }
            }
            return Some(meta.clone());
        }

        let provider_name =
            validated_provider_hint.or_else(|| guess_provider_for_custom_model(canonical_name))?;

        Some(synthesize_custom_model_meta(canonical_name, provider_name))
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

// ---------------------------------------------------------------------------
// Built-in model catalogue
// ---------------------------------------------------------------------------

fn builtin_models() -> Vec<ModelMeta> {
    let mut models = vec![
        // -- Anthropic --
        // Latest: Sonnet 4.6 (released 2026-02)
        ModelMeta {
            id: "claude-sonnet-4-6".into(),
            provider: "anthropic".into(),
            name: "Claude Sonnet 4.6".into(),
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.3,
                cache_write_per_mtok: 3.75,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        // Latest: Haiku 4.5 (released 2025-10)
        ModelMeta {
            id: "claude-haiku-4-5-20251001".into(),
            provider: "anthropic".into(),
            name: "Claude Haiku 4.5".into(),
            context_window: 200_000,
            max_output_tokens: 64_000,
            pricing: ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 5.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 1.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        // Latest: Opus 4.6 (released 2026-02)
        ModelMeta {
            id: "claude-opus-4-6".into(),
            provider: "anthropic".into(),
            name: "Claude Opus 4.6".into(),
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 25.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 6.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        // -- Google --
        ModelMeta {
            id: "gemini-2.5-pro".into(),
            provider: "google".into(),
            name: "Gemini 2.5 Pro".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing {
                input_per_mtok: 1.25,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 0.125,
                cache_write_per_mtok: 1.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gemini-2.5-flash".into(),
            provider: "google".into(),
            name: "Gemini 2.5 Flash".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing {
                input_per_mtok: 0.30,
                output_per_mtok: 2.50,
                cache_read_per_mtok: 0.03,
                cache_write_per_mtok: 0.30,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        // -- DeepSeek --
        ModelMeta {
            id: "deepseek-chat".into(),
            provider: "deepseek".into(),
            name: "DeepSeek V3".into(),
            context_window: 64_000,
            max_output_tokens: 8_192,
            pricing: ModelPricing {
                input_per_mtok: 0.27,
                output_per_mtok: 1.10,
                cache_read_per_mtok: 0.07,
                cache_write_per_mtok: 0.27,
            },
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "deepseek-reasoner".into(),
            provider: "deepseek".into(),
            name: "DeepSeek R1".into(),
            context_window: 64_000,
            max_output_tokens: 8_192,
            pricing: ModelPricing {
                input_per_mtok: 0.55,
                output_per_mtok: 2.19,
                cache_read_per_mtok: 0.14,
                cache_write_per_mtok: 0.55,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: false,
            },
        },
        // -- Moonshot / Kimi --
        ModelMeta {
            id: "kimi-k2.6".into(),
            provider: "moonshot".into(),
            name: "Kimi K2.6".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2.7-code".into(),
            provider: "moonshot".into(),
            name: "Kimi K2.7 Code".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2.7-code-highspeed".into(),
            provider: "moonshot".into(),
            name: "Kimi K2.7 Code Highspeed".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2.5".into(),
            provider: "moonshot".into(),
            name: "Kimi K2.5".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2-0905-preview".into(),
            provider: "moonshot".into(),
            name: "Kimi K2 0905 Preview".into(),
            context_window: 256_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2-turbo-preview".into(),
            provider: "moonshot".into(),
            name: "Kimi K2 Turbo Preview".into(),
            context_window: 256_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2-thinking".into(),
            provider: "moonshot".into(),
            name: "Kimi K2 Thinking".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-k2-thinking-turbo".into(),
            provider: "moonshot".into(),
            name: "Kimi K2 Thinking Turbo".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        // -- Kimi Code --
        ModelMeta {
            id: "kimi2.6".into(),
            provider: "kimi-code".into(),
            name: "Kimi K2.6 Code".into(),
            context_window: 262_144,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi2.7".into(),
            provider: "kimi-code".into(),
            name: "Kimi K2.7 Code".into(),
            context_window: 262_144,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "kimi-for-coding".into(),
            provider: "kimi-code".into(),
            name: "Kimi for Coding".into(),
            context_window: 262_144,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        // -- Z.AI --
        ModelMeta {
            id: "glm-4.7".into(),
            provider: "zai".into(),
            name: "GLM 4.7".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "glm-4.7-flash".into(),
            provider: "zai".into(),
            name: "GLM 4.7 Flash".into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "glm-5".into(),
            provider: "zai".into(),
            name: "GLM 5".into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        // -- Groq --
        ModelMeta {
            id: "google/gemini-3.1-flash-lite-preview".into(),
            provider: "openrouter".into(),
            name: "Google Gemini 3.1 Flash Lite Preview".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "google/gemini-3-flash-preview".into(),
            provider: "openrouter".into(),
            name: "Google Gemini 3 Flash Preview".into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "llama-3.3-70b-versatile".into(),
            provider: "groq".into(),
            name: "Llama 3.3 70B".into(),
            context_window: 128_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing {
                input_per_mtok: 0.59,
                output_per_mtok: 0.79,
                cache_read_per_mtok: 0.0,
                cache_write_per_mtok: 0.0,
            },
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
    ];

    let openai_insert_at = models
        .iter()
        .take_while(|model| model.provider == "anthropic")
        .count();
    models.splice(openai_insert_at..openai_insert_at, builtin_openai_models());
    models
}

pub fn builtin_openai_models() -> Vec<ModelMeta> {
    vec![
        ModelMeta {
            id: "gpt-5.6".into(),
            provider: "openai".into(),
            name: "GPT-5.6 Sol".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 30.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 6.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.6-sol".into(),
            provider: "openai".into(),
            name: "GPT-5.6 Sol".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 30.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 6.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.6-terra".into(),
            provider: "openai".into(),
            name: "GPT-5.6 Terra".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.25,
                cache_write_per_mtok: 3.125,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.6-luna".into(),
            provider: "openai".into(),
            name: "GPT-5.6 Luna".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 6.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 1.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.4".into(),
            provider: "openai".into(),
            name: "GPT-5.4".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.25,
                cache_write_per_mtok: 2.5,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.4-mini".into(),
            provider: "openai".into(),
            name: "GPT-5.4 mini".into(),
            context_window: 400_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 0.75,
                output_per_mtok: 4.5,
                cache_read_per_mtok: 0.075,
                cache_write_per_mtok: 0.75,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.4-nano".into(),
            provider: "openai".into(),
            name: "GPT-5.4 nano".into(),
            context_window: 400_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 0.20,
                output_per_mtok: 1.25,
                cache_read_per_mtok: 0.02,
                cache_write_per_mtok: 0.20,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.3-chat-latest".into(),
            provider: "openai".into(),
            name: "GPT-5.3 ChatGPT".into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing {
                input_per_mtok: 1.75,
                output_per_mtok: 14.0,
                cache_read_per_mtok: 0.175,
                cache_write_per_mtok: 1.75,
            },
            capabilities: Capabilities {
                reasoning: false,
                images: true,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.3-codex".into(),
            provider: "openai".into(),
            name: "GPT-5.3 Codex".into(),
            context_window: 400_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 1.75,
                output_per_mtok: 14.0,
                cache_read_per_mtok: 0.175,
                cache_write_per_mtok: 1.75,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        ModelMeta {
            id: "gpt-5.3-codex-spark".into(),
            provider: "openai".into(),
            name: "GPT-5.3 Codex Spark".into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
    ]
}

pub fn builtin_openai_codex_models() -> Vec<ModelMeta> {
    let mut models: Vec<ModelMeta> = builtin_openai_models()
        .into_iter()
        .map(|mut model| {
            model.provider = "openai-codex".into();
            model
        })
        .collect();

    models.push(ModelMeta {
        id: "gpt-5.5".into(),
        provider: "openai-codex".into(),
        name: "GPT-5.5".into(),
        context_window: 1_050_000,
        max_output_tokens: 128_000,
        pricing: ModelPricing::default(),
        capabilities: Capabilities {
            reasoning: true,
            images: true,
            tool_use: true,
        },
    });

    models
}

fn guess_provider_for_custom_model(model_name: &str) -> Option<&'static str> {
    let lower = model_name.to_lowercase();

    if lower.starts_with("gpt-")
        || lower.starts_with("chatgpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.contains("codex")
    {
        return Some("openai");
    }

    if lower.starts_with("claude") {
        return Some("anthropic");
    }

    if lower.starts_with("gemini") {
        return Some("google");
    }

    if lower.starts_with("kimi") || lower.starts_with("moonshot") {
        return Some("moonshot");
    }

    if lower.starts_with("glm-") || lower.starts_with("zai") || lower.starts_with("z-ai") {
        return Some("zai");
    }

    None
}

fn synthesize_custom_model_meta(model_id: &str, provider: &str) -> ModelMeta {
    match provider {
        "openai" => synthesize_openai_model_meta(model_id),
        "openai-codex" => {
            let mut meta = synthesize_openai_model_meta(model_id);
            meta.provider = "openai-codex".into();
            meta
        }
        "anthropic" => ModelMeta {
            id: model_id.into(),
            provider: provider.into(),
            name: model_id.into(),
            context_window: 200_000,
            max_output_tokens: 64_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        "google" => ModelMeta {
            id: model_id.into(),
            provider: provider.into(),
            name: model_id.into(),
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        "moonshot" => ModelMeta {
            id: model_id.into(),
            provider: provider.into(),
            name: model_id.into(),
            context_window: 256_000,
            max_output_tokens: if model_id.contains("thinking")
                || model_id.starts_with("kimi-k2.7-code")
                || matches!(model_id, "kimi-k2.6" | "kimi-k2.5")
            {
                32_768
            } else {
                16_384
            },
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        "zai" => ModelMeta {
            id: model_id.into(),
            provider: provider.into(),
            name: model_id.into(),
            context_window: 256_000,
            max_output_tokens: 32_768,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        _ => ModelMeta {
            id: model_id.into(),
            provider: provider.into(),
            name: model_id.into(),
            context_window: 200_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: false,
                images: false,
                tool_use: true,
            },
        },
    }
}

fn synthesize_openai_model_meta(model_id: &str) -> ModelMeta {
    match model_id {
        "gpt-4o" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-4o (legacy custom)".into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
                cache_read_per_mtok: 1.25,
                cache_write_per_mtok: 2.5,
            },
            capabilities: Capabilities {
                reasoning: false,
                images: true,
                tool_use: true,
            },
        },
        "o3" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "o3 (legacy custom)".into(),
            context_window: 200_000,
            max_output_tokens: 100_000,
            pricing: ModelPricing {
                input_per_mtok: 2.0,
                output_per_mtok: 8.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 2.0,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        "o4-mini" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "o4-mini (legacy custom)".into(),
            context_window: 200_000,
            max_output_tokens: 100_000,
            pricing: ModelPricing {
                input_per_mtok: 1.1,
                output_per_mtok: 4.4,
                cache_read_per_mtok: 0.275,
                cache_write_per_mtok: 1.1,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        "gpt-5.3-codex-spark" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-5.3 Codex Spark (preview)".into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        _ if model_id.starts_with("gpt-5.3-codex") || model_id.contains("codex") => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: model_id.into(),
            context_window: 400_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        _ if model_id.contains("chat-latest") => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: model_id.into(),
            context_window: 128_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: false,
                images: true,
                tool_use: true,
            },
        },
        _ if matches!(model_id, "gpt-5.6" | "gpt-5.6-sol") => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-5.6 Sol".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 5.0,
                output_per_mtok: 30.0,
                cache_read_per_mtok: 0.5,
                cache_write_per_mtok: 6.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ if model_id == "gpt-5.6-terra" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-5.6 Terra".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.25,
                cache_write_per_mtok: 3.125,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ if model_id == "gpt-5.6-luna" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-5.6 Luna".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing {
                input_per_mtok: 1.0,
                output_per_mtok: 6.0,
                cache_read_per_mtok: 0.1,
                cache_write_per_mtok: 1.25,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ if model_id == "gpt-5.5" => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: "GPT-5.5".into(),
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ if model_id.starts_with("gpt-5") => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: model_id.into(),
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ if model_id.starts_with('o') => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: model_id.into(),
            context_window: 200_000,
            max_output_tokens: 100_000,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: true,
                images: true,
                tool_use: true,
            },
        },
        _ => ModelMeta {
            id: model_id.into(),
            provider: "openai".into(),
            name: model_id.into(),
            context_window: 200_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing::default(),
            capabilities: Capabilities {
                reasoning: false,
                images: true,
                tool_use: true,
            },
        },
    }
}

fn builtin_aliases() -> Vec<(String, String)> {
    vec![
        // Anthropic — sonnet
        ("sonnet".into(), "claude-sonnet-4-6".into()),
        ("claude-sonnet".into(), "claude-sonnet-4-6".into()),
        ("sonnet-4.6".into(), "claude-sonnet-4-6".into()),
        // Anthropic — haiku
        ("haiku".into(), "claude-haiku-4-5-20251001".into()),
        ("claude-haiku".into(), "claude-haiku-4-5-20251001".into()),
        ("haiku-4.5".into(), "claude-haiku-4-5-20251001".into()),
        // Anthropic — opus
        ("opus".into(), "claude-opus-4-6".into()),
        ("claude-opus".into(), "claude-opus-4-6".into()),
        ("opus-4.6".into(), "claude-opus-4-6".into()),
        // OpenAI
        ("gpt5.6".into(), "gpt-5.6".into()),
        ("gpt-5.6".into(), "gpt-5.6".into()),
        ("gpt5.6sol".into(), "gpt-5.6-sol".into()),
        ("gpt-5.6-sol".into(), "gpt-5.6-sol".into()),
        ("gpt5.6terra".into(), "gpt-5.6-terra".into()),
        ("gpt-5.6-terra".into(), "gpt-5.6-terra".into()),
        ("gpt5.6luna".into(), "gpt-5.6-luna".into()),
        ("gpt-5.6-luna".into(), "gpt-5.6-luna".into()),
        ("gpt5.5".into(), "gpt-5.5".into()),
        ("gpt-5.5".into(), "gpt-5.5".into()),
        ("chatgpt5.5".into(), "gpt-5.5".into()),
        ("chatgpt-5.5".into(), "gpt-5.5".into()),
        ("gpt5".into(), "gpt-5.4".into()),
        ("gpt5.4".into(), "gpt-5.4".into()),
        ("gpt-5".into(), "gpt-5.4".into()),
        ("gpt-5.4".into(), "gpt-5.4".into()),
        ("gpt5mini".into(), "gpt-5.4-mini".into()),
        ("gpt-5-mini".into(), "gpt-5.4-mini".into()),
        ("gpt5nano".into(), "gpt-5.4-nano".into()),
        ("gpt-5-nano".into(), "gpt-5.4-nano".into()),
        ("chatgpt".into(), "gpt-5.3-chat-latest".into()),
        ("chatgpt-latest".into(), "gpt-5.3-chat-latest".into()),
        ("gpt5chat".into(), "gpt-5.3-chat-latest".into()),
        ("codex".into(), "gpt-5.3-codex".into()),
        ("gpt5codex".into(), "gpt-5.3-codex".into()),
        ("spark".into(), "gpt-5.3-codex-spark".into()),
        ("codex-spark".into(), "gpt-5.3-codex-spark".into()),
        // Google
        ("gemini-pro".into(), "gemini-2.5-pro".into()),
        ("gemini-flash".into(), "gemini-2.5-flash".into()),
        // DeepSeek
        ("deepseek".into(), "deepseek-chat".into()),
        ("deepseek-v3".into(), "deepseek-chat".into()),
        ("deepseek-r1".into(), "deepseek-reasoner".into()),
        // Moonshot / Kimi
        ("kimi".into(), "kimi-k2.6".into()),
        ("kimi-k2.6".into(), "kimi-k2.6".into()),
        ("kimi-k2.7".into(), "kimi-k2.7-code".into()),
        ("kimi-k2.7-code".into(), "kimi-k2.7-code".into()),
        (
            "kimi-k2.7-code-highspeed".into(),
            "kimi-k2.7-code-highspeed".into(),
        ),
        ("kimi-k2.5".into(), "kimi-k2.5".into()),
        ("kimi-k2".into(), "kimi-k2-0905-preview".into()),
        ("kimi-k2-0905".into(), "kimi-k2-0905-preview".into()),
        ("kimi-k2-turbo".into(), "kimi-k2-turbo-preview".into()),
        ("kimi-thinking".into(), "kimi-k2-thinking".into()),
        ("kimi-k2-thinking".into(), "kimi-k2-thinking".into()),
        (
            "kimi-thinking-turbo".into(),
            "kimi-k2-thinking-turbo".into(),
        ),
        (
            "kimi-k2-thinking-turbo".into(),
            "kimi-k2-thinking-turbo".into(),
        ),
        // Kimi Code
        ("kimi-code".into(), "kimi2.7".into()),
        ("kimi2.6".into(), "kimi2.6".into()),
        ("kimi2.7".into(), "kimi2.7".into()),
        ("kimi-for-coding".into(), "kimi-for-coding".into()),
        // Groq
        ("zai".into(), "glm-4.7".into()),
        ("zai-glm".into(), "glm-4.7".into()),
        ("glm".into(), "glm-4.7".into()),
        ("glm-4.7".into(), "glm-4.7".into()),
        ("glm-4.7-flash".into(), "glm-4.7-flash".into()),
        ("glm-5".into(), "glm-5".into()),
        ("llama-groq".into(), "llama-3.3-70b-versatile".into()),
    ]
}

#[cfg(test)]
#[path = "model/tests.rs"]
mod tests;
