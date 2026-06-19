use std::collections::HashMap;
use std::io::{self, IsTerminal};

use imp_core::config::Config;
use imp_core::tools::web::types::SearchProvider;
use imp_llm::auth::{AuthStore, StoredCredential};
use imp_llm::model::{ModelMeta, ModelRegistry, ProviderMeta, ProviderRegistry};
use imp_llm::oauth::anthropic::AnthropicOAuth;
use imp_llm::oauth::chatgpt::ChatGptOAuth;
use imp_llm::oauth::kimi_code::KimiCodeOAuth;
use imp_llm::ThinkingLevel;

use crate::provider_secrets::{
    prompt_for_secret_fields, provider_alias, search_provider_docs_url, search_provider_from_name,
};
use crate::{prompt_input_line, save_user_config, thinking_level_label, web_search_provider_label};

fn oauth_login_success_message(auth_store: &AuthStore, provider: &str) -> String {
    auth_store
        .oauth_display_info(provider)
        .map(|info| info.login_message(provider))
        .unwrap_or_else(|| format!("Logged in to {provider} successfully."))
}

fn kimi_api_login_success_message(auth_store: &AuthStore) -> String {
    let registry = ProviderRegistry::with_builtins();
    let provider = registry
        .find("moonshot")
        .expect("moonshot provider should exist");

    let auth_kind = match auth_store.stored.get("moonshot") {
        Some(StoredCredential::SecretFields { fields })
            if fields.len() == 1 && fields.first().map(String::as_str) == Some("api_key") =>
        {
            "API key"
        }
        Some(StoredCredential::ApiKey { .. }) => "API key",
        Some(StoredCredential::SecretFields { .. }) => "credentials",
        Some(StoredCredential::OAuth(_)) => "credentials",
        None => "credentials",
    };

    format!(
        "Configured {} in imp's secure auth store using a {}. You can now run `imp -m kimi` or `imp -m kimi-k2.6`.",
        provider.name, auth_kind
    )
}

pub(crate) async fn run_web_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let provider = search_provider_from_name(provider_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Unknown web provider: {provider_name}. Use one of: tavily, exa, linkup, perplexity"
            ),
        )
    })?;

    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let _env_key = provider.env_key_name();
    let fields = prompt_for_secret_fields(
        provider.name(),
        provider.name(),
        search_provider_docs_url(provider),
    )?;

    auth_store.store_secret_fields(provider.name(), fields)?;
    eprintln!(
        "Credentials saved for {} in secure imp auth storage (metadata: {}).",
        provider.name(),
        auth_path.display()
    );
    eprintln!(
        "The web tool will now auto-detect {} without requiring an exported env var.",
        provider.name()
    );

    Ok(())
}

fn try_import_kimi_cli_credentials() -> Option<imp_llm::auth::OAuthCredential> {
    let path = std::path::PathBuf::from(std::env::var_os("HOME")?)
        .join(".kimi")
        .join("credentials")
        .join("kimi-code.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let access_token = json["access_token"].as_str()?.to_string();
    let refresh_token = json["refresh_token"].as_str()?.to_string();
    let expires_at = json["expires_at"].as_f64()? as u64;
    Some(imp_llm::auth::OAuthCredential {
        access_token,
        refresh_token,
        expires_at,
    })
}

pub(crate) async fn run_login_command(
    provider: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider_name = match provider {
        Some(provider) if !provider.trim().is_empty() => provider.trim().to_string(),
        _ => prompt_login_provider()?,
    };
    run_login(&provider_name).await
}

fn prompt_login_provider() -> Result<String, Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No provider specified. Use `imp login <provider>` with one of: anthropic, openai, kimi, kimi-code.",
        )
        .into());
    }

    let providers = [
        ("anthropic", "Anthropic / Claude"),
        ("openai", "OpenAI"),
        ("kimi", "Kimi Code"),
    ];
    println!("Choose an OAuth provider:");
    for (idx, (_id, label)) in providers.iter().enumerate() {
        println!("{}. {label}", idx + 1);
    }
    println!("Or type a provider id: anthropic, openai, kimi, kimi-code");

    let choice = prompt_input_line("Provider> ")?;
    let trimmed = choice.trim();
    if let Some((id, _label)) = providers
        .iter()
        .find(|(id, _)| trimmed.eq_ignore_ascii_case(id))
    {
        return Ok((*id).to_string());
    }
    if let Some((id, _label)) = trimmed
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|idx| providers.get(idx))
    {
        return Ok((*id).to_string());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid provider selection. Use one of: anthropic, openai, kimi, kimi-code.",
    )
    .into())
}

async fn run_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let canonical_provider = provider_alias(provider_name);
    // For login, "kimi" maps to the Kimi Code OAuth flow rather than Moonshot API key.
    let login_provider = if provider_name.trim().eq_ignore_ascii_case("kimi") {
        "kimi-code"
    } else {
        &canonical_provider
    };

    if login_provider == "anthropic" {
        let oauth = AnthropicOAuth::new();

        eprintln!("Opening browser for Anthropic login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                || async {
                    eprintln!("Paste the authorization code or redirect URL:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).ok()?;
                    let trimmed = input.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                },
            )
            .await?;

        auth_store.store(
            "anthropic",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!("{}", oauth_login_success_message(&auth_store, "anthropic"));
    } else if login_provider == "openai" || login_provider == "openai-codex" {
        let oauth = ChatGptOAuth::new();

        eprintln!("Opening browser for OpenAI login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                || async {
                    eprintln!("Paste the authorization code or redirect URL:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).ok()?;
                    let trimmed = input.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                },
            )
            .await?;

        auth_store.store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(credential.clone()),
        )?;
        auth_store.store(
            "openai-codex",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!(
            "{}",
            oauth_login_success_message(&auth_store, "openai-codex")
        );
    } else if login_provider == "kimi-code" {
        // Try importing existing kimi-cli credentials first.
        if let Some(credential) = try_import_kimi_cli_credentials() {
            auth_store.store(
                "kimi-code",
                imp_llm::auth::StoredCredential::OAuth(credential),
            )?;
            eprintln!("Imported Kimi Code credentials from kimi-cli.");
            eprintln!("{}", oauth_login_success_message(&auth_store, "kimi-code"));
            return Ok(());
        }

        let oauth = KimiCodeOAuth::new();

        eprintln!("Opening browser for Kimi Code login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                |msg| {
                    eprintln!("{msg}");
                },
            )
            .await?;

        auth_store.store(
            "kimi-code",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!("{}", oauth_login_success_message(&auth_store, "kimi-code"));
    } else if canonical_provider == "moonshot" {
        let registry = ProviderRegistry::with_builtins();
        let provider = registry
            .find("moonshot")
            .expect("moonshot provider should exist");

        eprintln!("Kimi uses a generated API key, not an OAuth browser flow in imp.");
        eprintln!(
            "Open {} to create a key from Moonshot / Kimi or Kimi Code, then paste it below.",
            provider.docs_url
        );
        let _ = open_url(&format!("https://{}", provider.docs_url));

        let value = prompt_input_line("api_key> ")?;
        if value.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "No api_key entered. Aborting.",
            )
            .into());
        }
        auth_store
            .store_secret_fields("moonshot", HashMap::from([("api_key".to_string(), value)]))?;
        eprintln!("{}", kimi_api_login_success_message(&auth_store));
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`imp login {provider_name}` is not supported. Use one of: anthropic, openai, kimi. For API-only providers, use `imp secrets {}`.",
                provider_alias(provider_name)
            ),
        )
        .into());
    }

    Ok(())
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()?;
    }
    Ok(())
}

fn provider_has_auth(auth_store: &AuthStore, meta: &ProviderMeta) -> bool {
    meta.env_vars.iter().any(|v| {
        std::env::var(v)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    }) || auth_store.has_credentials(meta.id)
        || (meta.id == "moonshot" && auth_store.has_credentials("kimi-code"))
}

fn save_auth_secret_fields(
    provider: &str,
    fields: HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
    auth_store.store_secret_fields(provider, fields)?;
    Ok(())
}

fn setup_visible_provider(provider_id: &str) -> bool {
    provider_id != "kimi-code"
}

fn setup_models_for_provider(registry: &ModelRegistry, provider_id: &str) -> Vec<ModelMeta> {
    let mut models: Vec<ModelMeta> = registry
        .list_by_provider(provider_id)
        .into_iter()
        .cloned()
        .collect();

    if provider_id == "openai" {
        for mut model in imp_llm::model::builtin_openai_codex_models() {
            if models.iter().any(|existing| existing.id == model.id) {
                continue;
            }
            model.provider = "openai".into();
            models.push(model);
        }
    }

    models
}

pub(crate) async fn run_setup_mode() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let cwd = std::env::current_dir()?;
    let mut config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let provider_registry = ProviderRegistry::with_builtins();
    let model_registry = ModelRegistry::with_builtins();

    println!("imp setup");
    println!("=========");

    let providers: Vec<&ProviderMeta> = provider_registry
        .list()
        .iter()
        .filter(|provider| setup_visible_provider(provider.id))
        .collect();
    println!("Providers:");
    for (idx, provider) in providers.iter().enumerate() {
        let status = if provider_has_auth(&auth_store, provider) {
            "configured"
        } else {
            "needs auth"
        };
        println!("{}. {} [{}]", idx + 1, provider.name, status);
    }
    let provider_choice = prompt_input_line("Select provider> ")?;
    let provider_index = provider_choice
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|idx| *idx < providers.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid provider selection"))?;
    let provider = &providers[provider_index];

    if !provider_has_auth(&auth_store, provider) {
        println!("No auth detected for {}.", provider.name);
        if provider.id == "anthropic" || provider.id == "openai" || provider.id == "moonshot" {
            let mode = prompt_input_line("Choose auth mode [login|key]> ")?;
            if mode.eq_ignore_ascii_case("login") {
                run_login(provider.id).await?;
            } else {
                println!("Enter api_key for {}", provider.name);
                let value = prompt_input_line("api_key> ")?;
                save_auth_secret_fields(
                    provider.id,
                    HashMap::from([("api_key".to_string(), value)]),
                )?;
            }
        } else {
            println!("Get a key at: {}", provider.docs_url);
            let value = prompt_input_line("api_key> ")?;
            save_auth_secret_fields(provider.id, HashMap::from([("api_key".to_string(), value)]))?;
        }
    }

    let models = setup_models_for_provider(&model_registry, provider.id);
    if models.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No built-in models found for provider {}", provider.id),
        )
        .into());
    }

    println!();
    println!("Models for {}:", provider.name);
    for (idx, model) in models.iter().enumerate().take(20) {
        println!("{}. {} ({})", idx + 1, model.name, model.id);
    }
    let model_choice = prompt_input_line("Select model> ")?;
    let model_index = model_choice
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|idx| *idx < models.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid model selection"))?;
    let model = &models[model_index];
    config.model = Some(model.id.clone());

    println!();
    println!("Thinking levels:");
    println!("1. off");
    println!("2. minimal");
    println!("3. low");
    println!("4. medium");
    println!("5. high");
    println!("6. xhigh");
    let thinking_choice = prompt_input_line("Select thinking> ")?;
    config.thinking = Some(match thinking_choice.trim() {
        "1" => ThinkingLevel::Off,
        "2" => ThinkingLevel::Minimal,
        "3" => ThinkingLevel::Low,
        "4" => ThinkingLevel::Medium,
        "5" => ThinkingLevel::High,
        "6" => ThinkingLevel::XHigh,
        _ => {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid thinking selection").into(),
            )
        }
    });

    println!();
    println!("Web search provider:");
    println!("1. none");
    println!("2. tavily");
    println!("3. exa");
    println!("4. linkup");
    println!("5. perplexity");
    let web_choice = prompt_input_line("Select web search provider> ")?;
    config.web.search_provider = match web_choice.trim() {
        "1" | "" => None,
        "2" => Some(SearchProvider::Tavily),
        "3" => Some(SearchProvider::Exa),
        "4" => Some(SearchProvider::Linkup),
        "5" => Some(SearchProvider::Perplexity),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid web provider selection",
            )
            .into())
        }
    };

    if let Some(web_provider) = config.web.search_provider {
        let web_provider_name = web_provider.name();
        let web_has_auth = auth_store
            .resolve_secret_field(web_provider_name, "api_key")
            .is_ok()
            || std::env::var(web_provider.env_key_name()).is_ok();
        if !web_has_auth {
            println!("No key detected for web provider {}.", web_provider_name);
            println!("Get a key at: {}", search_provider_docs_url(web_provider));
            let value = prompt_input_line("api_key> ")?;
            save_auth_secret_fields(
                web_provider_name,
                HashMap::from([("api_key".to_string(), value)]),
            )?;
        }
    }

    let saved_path = save_user_config(&config)?;
    println!();
    println!("Setup complete.");
    println!("Config saved to {}", saved_path.display());
    println!("Provider: {}", provider.name);
    println!("Model: {}", model.id);
    println!(
        "Thinking: {}",
        config.thinking.map(thinking_level_label).unwrap_or("off")
    );
    println!(
        "Web search: {}",
        web_search_provider_label(config.web.search_provider)
    );

    Ok(())
}
