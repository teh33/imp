use super::*;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct MockSecretBackend {
    values: Mutex<HashMap<(String, String), String>>,
}

impl SecretBackend for MockSecretBackend {
    fn get(&self, provider: &str, field: &str) -> Result<Option<String>> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&(provider.to_string(), field.to_string()))
            .cloned())
    }

    fn set(&self, provider: &str, field: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert((provider.to_string(), field.to_string()), value.to_string());
        Ok(())
    }

    fn delete(&self, provider: &str, field: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .remove(&(provider.to_string(), field.to_string()));
        Ok(())
    }
}

struct FailingSetBackend;

impl SecretBackend for FailingSetBackend {
    fn get(&self, _provider: &str, _field: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn set(&self, provider: &str, field: &str, _value: &str) -> Result<()> {
        Err(crate::error::Error::Auth(format!(
            "test secure storage write failed for {provider}.{field}"
        )))
    }

    fn delete(&self, _provider: &str, _field: &str) -> Result<()> {
        Ok(())
    }
}

fn test_store(path: std::path::PathBuf) -> AuthStore {
    AuthStore::new_with_backend(path, Arc::new(MockSecretBackend::default()))
}

fn test_store_with_backend(path: std::path::PathBuf, backend: Arc<dyn SecretBackend>) -> AuthStore {
    AuthStore::new_with_backend(path, backend)
}

fn test_load_with_backend(path: &std::path::Path, backend: Arc<dyn SecretBackend>) -> AuthStore {
    AuthStore::load_with_backend(path, backend).unwrap()
}

fn jwt_with_openai_auth(plan: &str, account_id: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan,
            }
        })
        .to_string(),
    );
    format!("{header}.{payload}.signature")
}

#[test]
fn redact_oauth_error_body_removes_secret_fields() {
    let body = r#"{
        "error": "invalid_grant",
        "access_token": "access-secret",
        "nested": {"refresh_token": "refresh-secret"},
        "items": [{"id_token": "id-secret"}],
        "error_description": "safe detail"
    }"#;

    let redacted = redact_oauth_error_body(body);
    assert!(!redacted.contains("access-secret"));
    assert!(!redacted.contains("refresh-secret"));
    assert!(!redacted.contains("id-secret"));
    assert!(redacted.contains("safe detail"));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn test_oauth_credential_not_expired() {
    let cred = OAuthCredential {
        access_token: "token".into(),
        refresh_token: "refresh".into(),
        expires_at: crate::now() + 3600,
    };
    assert!(!cred.is_expired());
}

#[test]
fn test_oauth_credential_expired() {
    let cred = OAuthCredential {
        access_token: "token".into(),
        refresh_token: "refresh".into(),
        expires_at: crate::now().saturating_sub(100),
    };
    assert!(cred.is_expired());
}

#[test]
fn test_oauth_store_and_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    let cred = OAuthCredential {
        access_token: "sk-ant-access".into(),
        refresh_token: "rt-refresh".into(),
        expires_at: crate::now() + 3600,
    };
    store
        .store("anthropic", StoredCredential::OAuth(cred))
        .unwrap();

    let key = store.resolve("anthropic").unwrap();
    assert_eq!(key, "sk-ant-access");
}

#[test]
fn test_secure_secret_fields_store_and_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path.clone());
    let mut fields = HashMap::new();
    fields.insert("api_key".to_string(), "test-api".to_string());
    fields.insert("secret_key".to_string(), "test-secret".to_string());
    store.store_secret_fields("test-service", fields).unwrap();

    let data = std::fs::read_to_string(&path).unwrap();
    assert!(!data.contains("test-api"));
    assert!(!data.contains("test-secret"));
    assert_eq!(
        store
            .resolve_secret_field("test-service", "api_key")
            .unwrap(),
        "test-api"
    );
    assert_eq!(
        store
            .resolve_secret_field("test-service", "secret_key")
            .unwrap(),
        "test-secret"
    );
}

#[test]
fn store_secret_fields_does_not_save_metadata_when_secure_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store_with_backend(path.clone(), Arc::new(FailingSetBackend));

    let result = store.store_secret_fields(
        "google",
        HashMap::from([("api_key".to_string(), "test-api".to_string())]),
    );

    assert!(result.is_err());
    assert!(!store.stored.contains_key("google"));
    assert!(!path.exists());
}

#[test]
fn test_secure_secret_fields_persist_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let backend: Arc<dyn SecretBackend> = Arc::new(MockSecretBackend::default());
    let mut store = test_store_with_backend(path.clone(), Arc::clone(&backend));
    store
        .store_secret_fields(
            "test-service",
            HashMap::from([
                ("api_key".to_string(), "test-api".to_string()),
                ("secret_key".to_string(), "test-secret".to_string()),
            ]),
        )
        .unwrap();

    let loaded = test_load_with_backend(&path, backend);
    let resolved = loaded.resolve_secret_fields("test-service").unwrap();
    assert_eq!(
        resolved.get("api_key").map(String::as_str),
        Some("test-api")
    );
    assert_eq!(
        resolved.get("secret_key").map(String::as_str),
        Some("test-secret")
    );
}

#[test]
fn test_secure_remove_deletes_secret_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let backend: Arc<dyn SecretBackend> = Arc::new(MockSecretBackend::default());
    let mut store = test_store_with_backend(path, Arc::clone(&backend));
    store
        .store_secret_fields(
            "test-service",
            HashMap::from([
                ("api_key".to_string(), "test-api".to_string()),
                ("secret_key".to_string(), "test-secret".to_string()),
            ]),
        )
        .unwrap();

    store.remove("test-service").unwrap();
    assert!(store
        .resolve_secret_field("test-service", "api_key")
        .is_err());
    assert!(backend.get("test-service", "api_key").unwrap().is_none());
}

#[test]
fn test_oauth_detect_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    let fresh = OAuthCredential {
        access_token: "fresh".into(),
        refresh_token: "rt".into(),
        expires_at: crate::now() + 3600,
    };
    store
        .store("anthropic", StoredCredential::OAuth(fresh))
        .unwrap();
    assert!(!store.is_oauth_expired("anthropic"));

    let expired = OAuthCredential {
        access_token: "expired".into(),
        refresh_token: "rt".into(),
        expires_at: 0,
    };
    store
        .store("anthropic", StoredCredential::OAuth(expired))
        .unwrap();
    assert!(store.is_oauth_expired("anthropic"));
}

#[tokio::test]
async fn test_oauth_resolve_or_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    let expired = OAuthCredential {
        access_token: "old-access".into(),
        refresh_token: "rt-for-refresh".into(),
        expires_at: 0,
    };
    store
        .store("anthropic", StoredCredential::OAuth(expired))
        .unwrap();

    let key = store
        .resolve_or_refresh("anthropic", |refresh_tok| {
            let refresh_tok = refresh_tok.to_string();
            async move {
                assert_eq!(refresh_tok, "rt-for-refresh");
                Ok(OAuthCredential {
                    access_token: "new-access".into(),
                    refresh_token: "new-rt".into(),
                    expires_at: crate::now() + 3600,
                })
            }
        })
        .await
        .unwrap();

    assert_eq!(key, "new-access");
    let resolved = store.resolve("anthropic").unwrap();
    assert_eq!(resolved, "new-access");
}

#[tokio::test]
async fn test_oauth_resolve_or_refresh_not_expired() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    let fresh = OAuthCredential {
        access_token: "still-valid".into(),
        refresh_token: "rt".into(),
        expires_at: crate::now() + 3600,
    };
    store
        .store("anthropic", StoredCredential::OAuth(fresh))
        .unwrap();

    let key = store
        .resolve_or_refresh("anthropic", |_| async {
            panic!("refresh should not be called for non-expired token");
        })
        .await
        .unwrap();

    assert_eq!(key, "still-valid");
}

#[test]
fn test_load_invalid_auth_metadata_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{not valid json").unwrap();

    let backend: Arc<dyn SecretBackend> = Arc::new(MockSecretBackend::default());
    let err = match AuthStore::load_with_backend(&path, backend) {
        Ok(_) => panic!("invalid auth metadata should error"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(msg.contains("Failed to parse auth metadata"));
    assert!(msg.contains("auth.json"));
}

#[test]
fn test_save_writes_atomically_without_leaving_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path.clone());

    store
        .store(
            "openai",
            StoredCredential::ApiKey {
                key: "sk-atomic".into(),
            },
        )
        .unwrap();

    assert!(path.exists());
    assert!(!path.with_extension("json.tmp").exists());
    let loaded = test_load_with_backend(&path, Arc::new(MockSecretBackend::default()));
    assert_eq!(loaded.resolve("openai").unwrap(), "sk-atomic");
}

#[test]
fn test_oauth_store_persist_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let backend: Arc<dyn SecretBackend> = Arc::new(MockSecretBackend::default());

    {
        let mut store = test_store_with_backend(path.clone(), Arc::clone(&backend));
        let cred = OAuthCredential {
            access_token: "persisted-token".into(),
            refresh_token: "persisted-rt".into(),
            expires_at: crate::now() + 3600,
        };
        store
            .store("anthropic", StoredCredential::OAuth(cred))
            .unwrap();
    }

    let store = test_load_with_backend(&path, backend);
    let key = store.resolve("anthropic").unwrap();
    assert_eq!(key, "persisted-token");
}

#[test]
fn test_oauth_remove_credential() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    let cred = OAuthCredential {
        access_token: "to-remove".into(),
        refresh_token: "rt".into(),
        expires_at: crate::now() + 3600,
    };
    store
        .store("anthropic", StoredCredential::OAuth(cred))
        .unwrap();
    assert!(store.resolve("anthropic").is_ok());

    store.remove("anthropic").unwrap();
    std::env::remove_var("ANTHROPIC_API_KEY");
    assert!(store.resolve("anthropic").is_err());
}

#[test]
fn test_resolve_order_runtime_over_stored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "anthropic",
            StoredCredential::ApiKey {
                key: "stored-key".into(),
            },
        )
        .unwrap();

    store.set_runtime_key("anthropic", "runtime-key".into());
    let key = store.resolve("anthropic").unwrap();
    assert_eq!(key, "runtime-key");
}

#[test]
fn test_set_runtime_key_ignores_empty_or_whitespace_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store.set_runtime_key("openai", "runtime-key".into());
    assert_eq!(store.resolve("openai").unwrap(), "runtime-key");

    store.set_runtime_key("openai", "   ".into());
    assert!(store.resolve("openai").is_err());
}

#[test]
fn test_resolve_stored_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "openai",
            StoredCredential::ApiKey {
                key: "sk-stored".into(),
            },
        )
        .unwrap();

    let key = store.resolve("openai").unwrap();
    assert_eq!(key, "sk-stored");
}

#[test]
fn test_resolve_env_secret_uses_moonshot_env_vars() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let store = test_store(path);

    std::env::remove_var("KIMI_API_KEY");
    std::env::set_var("MOONSHOT_API_KEY", "moonshot-env-key");
    let key = store.resolve("moonshot").unwrap();
    assert_eq!(key, "moonshot-env-key");
    std::env::remove_var("MOONSHOT_API_KEY");

    std::env::set_var("KIMI_API_KEY", "kimi-env-key");
    let key = store.resolve("moonshot").unwrap();
    assert_eq!(key, "kimi-env-key");
    std::env::remove_var("KIMI_API_KEY");
}

#[test]
fn test_resolve_api_key_only_ignores_oauth_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "openai",
            StoredCredential::OAuth(OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: crate::now() + 3600,
            }),
        )
        .unwrap();

    assert!(store.resolve_api_key_only("openai").is_err());
}

#[tokio::test]
async fn test_resolve_chatgpt_oauth_prefers_openai_codex() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "openai",
            StoredCredential::OAuth(OAuthCredential {
                access_token: "openai-oauth".into(),
                refresh_token: "openai-refresh".into(),
                expires_at: crate::now() + 3600,
            }),
        )
        .unwrap();
    store
        .store(
            "openai-codex",
            StoredCredential::OAuth(OAuthCredential {
                access_token: "codex-oauth".into(),
                refresh_token: "codex-refresh".into(),
                expires_at: crate::now() + 3600,
            }),
        )
        .unwrap();

    let key = store.resolve_chatgpt_oauth().await.unwrap();
    assert_eq!(key, "codex-oauth");
}

#[tokio::test]
async fn test_resolve_chatgpt_oauth_falls_back_to_openai() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "openai",
            StoredCredential::OAuth(OAuthCredential {
                access_token: "openai-oauth".into(),
                refresh_token: "openai-refresh".into(),
                expires_at: crate::now() + 3600,
            }),
        )
        .unwrap();

    let key = store.resolve_chatgpt_oauth().await.unwrap();
    assert_eq!(key, "openai-oauth");
}

#[test]
fn test_oauth_display_info_for_openai_credential() {
    let credential = OAuthCredential {
        access_token: jwt_with_openai_auth("pro", "acct-12345678"),
        refresh_token: "refresh".into(),
        expires_at: crate::now() + 3600,
    };

    let info = oauth_display_info_for_credential("openai", &credential).unwrap();
    assert_eq!(info.account_id.as_deref(), Some("acct-12345678"));
    assert_eq!(info.plan.as_deref(), Some("pro"));
    assert_eq!(info.short_account_id().as_deref(), Some("acct-123…"));
}

#[test]
fn test_oauth_display_info_for_anthropic_credential() {
    let credential = OAuthCredential {
        access_token: "sk-ant-oat01-example".into(),
        refresh_token: "refresh".into(),
        expires_at: crate::now() + 3600,
    };

    let info = oauth_display_info_for_credential("anthropic", &credential).unwrap();
    assert_eq!(info.plan.as_deref(), Some("Claude Max/Pro"));
    assert!(info.account_id.is_none());
    assert_eq!(
        info.login_message("anthropic"),
        "Logged in to Anthropic with Claude Max/Pro subscription credentials."
    );
}

#[test]
fn test_remove_then_resolve_falls_through() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store(
            "google",
            StoredCredential::ApiKey {
                key: "google-key".into(),
            },
        )
        .unwrap();
    assert!(store.resolve("google").is_ok());

    store.remove("google").unwrap();
    std::env::remove_var("GOOGLE_API_KEY");
    let result = store.resolve("google");
    assert!(result.is_err());
}

#[test]
fn provider_lookup_candidates_include_legacy_render_casing() {
    assert_eq!(
        provider_lookup_candidates("render"),
        vec!["render".to_string(), "Render".to_string()]
    );
    assert_eq!(
        provider_lookup_candidates("Render"),
        vec!["Render".to_string(), "render".to_string()]
    );
}

#[test]
fn field_lookup_candidates_support_porkbun_secret_key_typo() {
    assert_eq!(
        field_lookup_candidates("secrets_key"),
        vec!["secrets_key".to_string(), "secret_key".to_string()]
    );
}

#[test]
fn resolve_secret_fields_uses_provider_alias_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);

    store
        .store_secret_fields(
            "Render",
            HashMap::from([("api_key".to_string(), "render-secret".to_string())]),
        )
        .unwrap();

    let fields = store.resolve_secret_fields("render").unwrap();
    assert_eq!(
        fields.get("api_key").map(String::as_str),
        Some("render-secret")
    );
}

#[test]
fn test_secret_status_reports_missing_keychain_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = test_store(path);
    store.stored.insert(
        "google".into(),
        StoredCredential::SecretFields {
            fields: vec!["api_key".into()],
        },
    );

    let status = store.secret_status("google").unwrap();
    assert_eq!(status.provider, "google");
    assert_eq!(
        status.fields,
        vec![("api_key".to_string(), SecretFieldStatus::Missing)]
    );
    assert!(!status.is_usable());
    assert!(!store.has_credentials("google"));
}

#[test]
fn test_has_credentials_detects_kimi_env_var() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let store = test_store(path);

    std::env::remove_var("MOONSHOT_API_KEY");
    std::env::set_var("KIMI_API_KEY", "kimi-env-key");
    assert!(store.has_credentials("moonshot"));
    std::env::remove_var("KIMI_API_KEY");
}

#[test]
fn test_unknown_provider_returns_auth_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let store = test_store(path);
    let result = store.resolve("unknown_provider");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, crate::error::Error::Auth(_)));
}
