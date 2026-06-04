use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::truncate_chars_with_suffix;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

pub type ApiKey = String;
const KEYRING_SERVICE: &str = "imp";
const LEGACY_KEYRING_SERVICES: &[&str] = &["imp-cli", "impeccable", "mana"];

fn provider_lookup_candidates(provider: &str) -> Vec<String> {
    let mut candidates = vec![provider.to_string()];
    let lower = provider.to_lowercase();
    if lower != provider {
        candidates.push(lower);
    }
    if provider == "render" {
        candidates.push("Render".to_string());
    }
    dedupe_strings(candidates)
}

fn field_lookup_candidates(field: &str) -> Vec<String> {
    let mut candidates = vec![field.to_string()];
    if field == "secrets_key" {
        candidates.push("secret_key".to_string());
    }
    if field == "secret_key" {
        candidates.push("secrets_key".to_string());
    }
    dedupe_strings(candidates)
}

fn dedupe_strings(values: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}

trait SecretBackend: Send + Sync {
    fn get(&self, provider: &str, field: &str) -> Result<Option<String>>;
    fn set(&self, provider: &str, field: &str, value: &str) -> Result<()>;
    fn delete(&self, provider: &str, field: &str) -> Result<()>;
}

struct KeyringBackend;

impl KeyringBackend {
    fn entry(service: &str, provider: &str, field: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(service, &format!("{provider}:{field}"))
            .map_err(|e| crate::error::Error::Auth(format!("Secure storage init failed: {e}")))
    }

    fn read_entry(service: &str, provider: &str, field: &str) -> Result<Option<String>> {
        let entry = Self::entry(service, provider, field)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Self::map_error("read", provider, field, error)),
        }
    }

    fn lookup_secret(provider: &str, field: &str) -> Result<Option<String>> {
        let providers = provider_lookup_candidates(provider);
        let fields = field_lookup_candidates(field);
        for candidate_provider in &providers {
            for candidate_field in &fields {
                if let Some(value) =
                    Self::read_entry(KEYRING_SERVICE, candidate_provider, candidate_field)?
                {
                    return Ok(Some(value));
                }
            }
        }
        for service in LEGACY_KEYRING_SERVICES {
            for candidate_provider in &providers {
                for candidate_field in &fields {
                    if let Some(value) =
                        Self::read_entry(service, candidate_provider, candidate_field)?
                    {
                        return Ok(Some(value));
                    }
                }
            }
        }
        Ok(None)
    }

    fn map_error(
        action: &str,
        provider: &str,
        field: &str,
        error: keyring::Error,
    ) -> crate::error::Error {
        crate::error::Error::Auth(format!(
            "Secure storage {action} failed for {provider}.{field}: {error}"
        ))
    }
}

impl SecretBackend for KeyringBackend {
    fn get(&self, provider: &str, field: &str) -> Result<Option<String>> {
        Self::lookup_secret(provider, field)
    }

    fn set(&self, provider: &str, field: &str, value: &str) -> Result<()> {
        let entry = Self::entry(KEYRING_SERVICE, provider, field)?;
        entry
            .set_password(value)
            .map_err(|error| Self::map_error("write", provider, field, error))?;

        match Self::read_entry(KEYRING_SERVICE, provider, field)? {
            Some(stored) if stored == value => Ok(()),
            Some(_) => Err(crate::error::Error::Auth(format!(
                "Secure storage write verification failed for {provider}.{field}: readback did not match"
            ))),
            None => Err(crate::error::Error::Auth(format!(
                "Secure storage write verification failed for {provider}.{field}: value was not readable after write"
            ))),
        }
    }

    fn delete(&self, provider: &str, field: &str) -> Result<()> {
        let providers = provider_lookup_candidates(provider);
        let fields = field_lookup_candidates(field);
        let mut first_error = None;
        for service in
            std::iter::once(KEYRING_SERVICE).chain(LEGACY_KEYRING_SERVICES.iter().copied())
        {
            for candidate_provider in &providers {
                for candidate_field in &fields {
                    let entry = Self::entry(service, candidate_provider, candidate_field)?;
                    match entry.delete_credential() {
                        Ok(()) | Err(keyring::Error::NoEntry) => {}
                        Err(error) if first_error.is_none() => {
                            first_error = Some(Self::map_error(
                                "delete",
                                candidate_provider,
                                candidate_field,
                                error,
                            ));
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
}

impl OAuthCredential {
    /// Check whether this token has expired (or will within the next minute).
    pub fn is_expired(&self) -> bool {
        crate::now() >= self.expires_at
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StoredCredential {
    ApiKey { key: String },
    OAuth(OAuthCredential),
    SecretFields { fields: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretFieldStatus {
    Present,
    Missing,
    Error(String),
}

impl SecretFieldStatus {
    #[must_use]
    pub fn is_present(&self) -> bool {
        matches!(self, Self::Present)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretStatus {
    pub provider: String,
    pub fields: Vec<(String, SecretFieldStatus)>,
}

impl SecretStatus {
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.fields.iter().all(|(_, status)| status.is_present())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthDisplayInfo {
    pub account_id: Option<String>,
    pub plan: Option<String>,
    pub using_subscription: bool,
}

impl OAuthDisplayInfo {
    pub fn login_message(&self, provider: &str) -> String {
        match provider {
            "openai" | "openai-codex" => {
                let mut message = String::from("Logged in to OpenAI / ChatGPT");
                if let Some(account_id) = &self.account_id {
                    message.push_str(&format!(" as account {account_id}"));
                }
                if let Some(plan) = &self.plan {
                    message.push_str(&format!(", plan: {plan}"));
                }
                message.push('.');
                message
            }
            "anthropic" => {
                if let Some(plan) = &self.plan {
                    format!("Logged in to Anthropic with {plan} subscription credentials.")
                } else {
                    "Logged in to Anthropic with OAuth subscription credentials.".into()
                }
            }
            _ => format!("Logged in to {provider} with OAuth credentials."),
        }
    }

    pub fn status_summary(&self) -> String {
        match (&self.plan, self.short_account_id()) {
            (Some(plan), Some(account_id)) => format!("{plan} · {account_id}"),
            (Some(plan), None) => plan.clone(),
            (None, Some(account_id)) => account_id,
            (None, None) if self.using_subscription => "subscription".into(),
            (None, None) => "oauth".into(),
        }
    }

    pub fn short_account_id(&self) -> Option<String> {
        self.account_id
            .as_ref()
            .map(|account_id| truncate_chars_with_suffix(account_id, 8, "…"))
    }
}

/// Manages API keys and OAuth credentials.
pub struct AuthStore {
    runtime_keys: HashMap<String, String>,
    pub stored: HashMap<String, StoredCredential>,
    path: PathBuf,
    backend: Arc<dyn SecretBackend>,
}

impl AuthStore {
    pub fn new(path: PathBuf) -> Self {
        Self::new_with_backend(path, Arc::new(KeyringBackend))
    }

    fn new_with_backend(path: PathBuf, backend: Arc<dyn SecretBackend>) -> Self {
        Self {
            runtime_keys: HashMap::new(),
            stored: HashMap::new(),
            path,
            backend,
        }
    }

    /// Load stored credentials from disk.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        Self::load_with_backend(path, Arc::new(KeyringBackend))
    }

    fn load_with_backend(path: &std::path::Path, backend: Arc<dyn SecretBackend>) -> Result<Self> {
        let stored = if path.exists() {
            let data = std::fs::read_to_string(path)?;
            serde_json::from_str(&data).map_err(|error| {
                crate::error::Error::Auth(format!(
                    "Failed to parse auth metadata at {}: {error}",
                    path.display()
                ))
            })?
        } else {
            HashMap::new()
        };
        Ok(Self {
            runtime_keys: HashMap::new(),
            stored,
            path: path.to_path_buf(),
            backend,
        })
    }

    /// Set a runtime override (not persisted).
    /// Empty or whitespace-only values are treated as absent.
    pub fn set_runtime_key(&mut self, provider: &str, key: String) {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            self.runtime_keys.remove(provider);
            return;
        }
        self.runtime_keys
            .insert(provider.to_string(), trimmed.to_string());
    }

    /// Check whether credentials are usable for a provider without producing an error.
    /// Returns true if a runtime key, readable stored credential, or env var is available.
    pub fn has_credentials(&self, provider: &str) -> bool {
        self.resolve(provider).is_ok()
    }

    /// Resolution order: runtime override -> stored -> env var -> error.
    pub fn resolve(&self, provider: &str) -> Result<ApiKey> {
        if let Some(key) = self.runtime_keys.get(provider) {
            return Ok(key.clone());
        }

        if let Some(StoredCredential::OAuth(oauth)) = self.stored.get(provider) {
            return Ok(oauth.access_token.clone());
        }

        self.resolve_secret_field(provider, "api_key")
    }

    /// Resolve an API key without falling back to stored OAuth credentials.
    pub fn resolve_api_key_only(&self, provider: &str) -> Result<ApiKey> {
        self.resolve_secret_field(provider, "api_key")
    }

    /// Resolve a named secret field for any stored provider/service.
    pub fn resolve_secret_field(&self, provider: &str, field: &str) -> Result<String> {
        if field == "api_key" {
            if let Some(key) = self.runtime_keys.get(provider) {
                return Ok(key.clone());
            }
        }

        if let Some((stored_provider, credential)) = self.stored_credential(provider) {
            match credential {
                StoredCredential::ApiKey { key } if field == "api_key" => return Ok(key.clone()),
                StoredCredential::SecretFields { fields } => {
                    if fields.iter().any(|name| name == field) {
                        return self
                            .backend
                            .get(stored_provider, field)?
                            .ok_or_else(|| missing_secret_error(stored_provider, field));
                    }
                }
                StoredCredential::OAuth(_) => {}
                StoredCredential::ApiKey { .. } => {}
            }
        }

        if let Some(value) = resolve_env_secret(provider, field) {
            return Ok(value);
        }

        Err(missing_secret_error(provider, field))
    }

    /// Store multiple named secret fields securely in the OS keychain and persist only metadata.
    pub fn store_secret_fields(
        &mut self,
        provider: &str,
        fields: HashMap<String, String>,
    ) -> Result<()> {
        if fields.is_empty() {
            return Err(crate::error::Error::Auth(format!(
                "No secret fields provided for {provider}."
            )));
        }

        let mut field_names = Vec::with_capacity(fields.len());
        for (field, value) in &fields {
            let field = field.trim();
            if field.is_empty() {
                return Err(crate::error::Error::Auth(format!(
                    "Secret field names for {provider} cannot be empty."
                )));
            }
            if value.trim().is_empty() {
                return Err(crate::error::Error::Auth(format!(
                    "Secret value for {provider}.{field} cannot be empty."
                )));
            }
            self.backend.set(provider, field, value)?;
            field_names.push(field.to_string());
        }

        field_names.sort();
        field_names.dedup();
        self.stored.insert(
            provider.to_string(),
            StoredCredential::SecretFields {
                fields: field_names,
            },
        );
        self.save()
    }

    fn stored_credential(&self, provider: &str) -> Option<(&str, &StoredCredential)> {
        provider_lookup_candidates(provider)
            .into_iter()
            .find_map(|candidate| {
                self.stored
                    .get_key_value(&candidate)
                    .map(|(stored_provider, credential)| (stored_provider.as_str(), credential))
            })
    }

    /// Check whether stored secret metadata points at readable secure-storage values.
    pub fn secret_status(&self, provider: &str) -> Option<SecretStatus> {
        let (stored_provider, credential) = self.stored_credential(provider)?;
        let fields = match credential {
            StoredCredential::SecretFields { fields } => fields
                .iter()
                .map(|field| {
                    let status = match self.backend.get(stored_provider, field) {
                        Ok(Some(value)) if !value.trim().is_empty() => SecretFieldStatus::Present,
                        Ok(_) => SecretFieldStatus::Missing,
                        Err(error) => SecretFieldStatus::Error(error.to_string()),
                    };
                    (field.clone(), status)
                })
                .collect(),
            StoredCredential::ApiKey { key } => vec![(
                "api_key".to_string(),
                if key.trim().is_empty() {
                    SecretFieldStatus::Missing
                } else {
                    SecretFieldStatus::Present
                },
            )],
            StoredCredential::OAuth(oauth) => vec![(
                "access_token".to_string(),
                if oauth.access_token.trim().is_empty() {
                    SecretFieldStatus::Missing
                } else {
                    SecretFieldStatus::Present
                },
            )],
        };

        Some(SecretStatus {
            provider: stored_provider.to_string(),
            fields,
        })
    }

    /// Resolve all stored secret fields for a provider into a map.
    pub fn resolve_secret_fields(&self, provider: &str) -> Result<HashMap<String, String>> {
        match self.stored_credential(provider) {
            Some((stored_provider, StoredCredential::SecretFields { fields })) => fields
                .iter()
                .map(|field| {
                    self.resolve_secret_field(stored_provider, field)
                        .map(|value| (field.clone(), value))
                })
                .collect(),
            Some((_stored_provider, StoredCredential::ApiKey { key })) => {
                Ok(HashMap::from([("api_key".to_string(), key.clone())]))
            }
            Some((_stored_provider, StoredCredential::OAuth(oauth))) => Ok(HashMap::from([(
                "access_token".to_string(),
                oauth.access_token.clone(),
            )])),
            None => {
                if let Some(api_key) = resolve_env_secret(provider, "api_key") {
                    Ok(HashMap::from([("api_key".to_string(), api_key)]))
                } else {
                    Err(missing_secret_error(provider, "api_key"))
                }
            }
        }
    }

    /// Resolve a ChatGPT/OpenAI OAuth token, preferring `openai-codex` when present.
    pub async fn resolve_chatgpt_oauth(&mut self) -> Result<ApiKey> {
        for provider in ["openai-codex", "openai"] {
            let Some(StoredCredential::OAuth(oauth)) = self.stored.get(provider) else {
                continue;
            };
            if oauth.is_expired() {
                return self
                    .resolve_or_refresh(provider, |refresh_token| {
                        let refresh_token = refresh_token.to_string();
                        async move {
                            crate::oauth::chatgpt::ChatGptOAuth::new()
                                .refresh_token(&refresh_token)
                                .await
                        }
                    })
                    .await;
            }
            return Ok(oauth.access_token.clone());
        }

        Err(crate::error::Error::Auth(
            "No ChatGPT OAuth credential found. Run `imp login openai` or configure an OpenAI API key."
                .into(),
        ))
    }

    pub fn oauth_display_info(&self, provider: &str) -> Option<OAuthDisplayInfo> {
        self.get_oauth(provider)
            .and_then(|credential| oauth_display_info_for_credential(provider, credential))
    }

    /// Store a credential and persist to disk.
    pub fn store(&mut self, provider: &str, credential: StoredCredential) -> Result<()> {
        self.stored.insert(provider.to_string(), credential);
        self.save()
    }

    /// Resolve API key, auto-refreshing expired OAuth tokens.
    /// Persists the refreshed credential to disk on success.
    pub async fn resolve_with_refresh(&mut self, provider: &str) -> Result<ApiKey> {
        if let Some(StoredCredential::OAuth(oauth)) = self.stored.get(provider) {
            if oauth.is_expired() {
                let refresh_token = oauth.refresh_token.clone();
                let result = match provider {
                    "anthropic" => {
                        crate::oauth::anthropic::AnthropicOAuth::new()
                            .refresh_token(&refresh_token)
                            .await
                    }
                    "kimi-code" => {
                        crate::oauth::kimi_code::KimiCodeOAuth::new()
                            .refresh_token(&refresh_token)
                            .await
                    }
                    _ => {
                        return Err(crate::error::Error::Auth(format!(
                            "OAuth refresh not implemented for provider: {provider}"
                        )));
                    }
                };
                match result {
                    Ok(new_cred) => {
                        self.store(provider, StoredCredential::OAuth(new_cred))?;
                    }
                    Err(e) => {
                        return Err(crate::error::Error::Auth(format!(
                            "Token refresh failed: {e}. Run `imp login` to re-authenticate."
                        )));
                    }
                }
            }
        }
        self.resolve(provider)
    }

    /// Check if the stored OAuth credential for a provider is expired.
    pub fn is_oauth_expired(&self, provider: &str) -> bool {
        matches!(
            self.stored.get(provider),
            Some(StoredCredential::OAuth(oauth)) if oauth.is_expired()
        )
    }

    /// Get the stored OAuth credential for a provider (if any).
    pub fn get_oauth(&self, provider: &str) -> Option<&OAuthCredential> {
        match self.stored.get(provider) {
            Some(StoredCredential::OAuth(oauth)) => Some(oauth),
            _ => None,
        }
    }

    /// Resolve API key with automatic OAuth refresh.
    pub async fn resolve_or_refresh<F, Fut>(
        &mut self,
        provider: &str,
        refresh_fn: F,
    ) -> Result<ApiKey>
    where
        F: FnOnce(&str) -> Fut,
        Fut: std::future::Future<Output = Result<OAuthCredential>>,
    {
        if let Some(StoredCredential::OAuth(oauth)) = self.stored.get(provider) {
            if oauth.is_expired() {
                let refresh_token = oauth.refresh_token.clone();
                let new_cred = refresh_fn(&refresh_token).await?;
                let access_token = new_cred.access_token.clone();
                self.store(provider, StoredCredential::OAuth(new_cred))?;
                return Ok(access_token);
            }
        }
        self.resolve(provider)
    }

    /// Remove a stored credential (logout).
    pub fn remove(&mut self, provider: &str) -> Result<()> {
        if let Some(StoredCredential::SecretFields { fields }) = self.stored.remove(provider) {
            for field in fields {
                self.backend.delete(provider, &field)?;
            }
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.stored)?;
        let temp_path = self.path.with_extension("json.tmp");
        std::fs::write(&temp_path, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&temp_path, perms);
        }
        std::fs::rename(&temp_path, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&self.path, perms);
        }
        Ok(())
    }
}

pub fn redact_provider_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(mut value) = serde_json::from_str::<Value>(trimmed) {
        redact_secret_json_fields(&mut value);
        return serde_json::to_string(&value)
            .unwrap_or_else(|_| "[redacted unparsable error body]".into());
    }
    redact_secret_like_pairs(trimmed)
}

fn redact_secret_json_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if is_secret_field_name(key) {
                    if !value.is_null() {
                        *value = Value::String("[REDACTED]".into());
                    }
                } else {
                    redact_secret_json_fields(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_secret_json_fields(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn is_secret_field_name(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "access_token"
            | "accesstoken"
            | "api_key"
            | "apikey"
            | "authorization"
            | "id_token"
            | "idtoken"
            | "refresh_token"
            | "refreshtoken"
            | "secret"
            | "token"
    ) || normalized.ends_with("token")
        || normalized.ends_with("secret")
        || normalized.ends_with("apikey")
}

fn redact_secret_like_pairs(body: &str) -> String {
    body.split_whitespace()
        .map(|part| {
            let lowered = part.to_ascii_lowercase();
            if [
                "access_token",
                "refresh_token",
                "id_token",
                "api_key",
                "token",
                "secret",
            ]
            .iter()
            .any(|key| lowered.starts_with(key) || lowered.contains(&format!("{key}=")))
            {
                "[REDACTED]".to_string()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn redact_oauth_error_body(body: &str) -> String {
    redact_provider_error_body(body)
}

fn resolve_env_secret(provider: &str, field: &str) -> Option<String> {
    if field == "api_key" {
        let registry = crate::model::ProviderRegistry::with_builtins();
        if let Some(meta) = registry.find(provider) {
            for env_var in meta.env_vars {
                if let Ok(value) = std::env::var(env_var) {
                    if !value.trim().is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }

    let env_var = env_var_name(provider, field);
    std::env::var(&env_var)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn env_var_name(provider: &str, field: &str) -> String {
    let provider = provider.to_uppercase().replace('-', "_");
    let field = field.to_uppercase().replace('-', "_");
    format!("{provider}_{field}")
}

fn missing_secret_error(provider: &str, field: &str) -> crate::error::Error {
    crate::error::Error::Auth(format!(
        "No readable secret field '{field}' found for {provider}. Set {} or run `imp secrets {provider}` to save it again.",
        env_var_name(provider, field)
    ))
}

pub fn oauth_display_info_for_credential(
    provider: &str,
    credential: &OAuthCredential,
) -> Option<OAuthDisplayInfo> {
    match provider {
        "anthropic" => Some(OAuthDisplayInfo {
            account_id: None,
            plan: Some("Claude Max/Pro".into()),
            using_subscription: true,
        }),
        "openai" | "openai-codex" => decode_openai_oauth_display_info(&credential.access_token),
        "kimi-code" => Some(OAuthDisplayInfo {
            account_id: None,
            plan: Some("Kimi Code".into()),
            using_subscription: true,
        }),
        _ => None,
    }
}

fn decode_openai_oauth_display_info(access_token: &str) -> Option<OAuthDisplayInfo> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    let auth = claims.get("https://api.openai.com/auth")?;

    Some(OAuthDisplayInfo {
        account_id: auth
            .get("chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        plan: auth
            .get("chatgpt_plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        using_subscription: true,
    })
}

#[cfg(test)]
#[path = "auth/tests.rs"]
mod tests;
