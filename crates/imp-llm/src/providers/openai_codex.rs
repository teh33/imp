use std::pin::Pin;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_core::Stream;
use serde_json::Value;

use crate::auth::{ApiKey, AuthStore};
use crate::error::{Error, Result};
use crate::model::{Model, ModelMeta};
use crate::provider::{Context, Provider, RequestOptions};
use crate::stream::StreamEvent;

use super::openai::{build_request_json, stream_response_json};

const CODEX_API_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// ChatGPT/Codex-backed OpenAI provider.
pub struct OpenAiCodexProvider {
    client: reqwest::Client,
    models: Vec<ModelMeta>,
}

impl Default for OpenAiCodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiCodexProvider {
    pub fn new() -> Self {
        Self {
            client: super::streaming_http_client(),
            models: crate::model::builtin_openai_codex_models(),
        }
    }
}

fn extract_account_id(token: &str) -> Result<String> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| Error::Auth("Invalid ChatGPT OAuth token".into()))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| Error::Auth("Failed to decode ChatGPT OAuth token".into()))?;
    let claims: Value = serde_json::from_slice(&decoded)
        .map_err(|_| Error::Auth("Failed to parse ChatGPT OAuth token claims".into()))?;

    claims
        .get(JWT_CLAIM_PATH)
        .and_then(|value| value.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Auth("ChatGPT OAuth token is missing chatgpt_account_id".into()))
}

fn strip_unsupported_codex_fields(request: &mut Value) {
    let Some(object) = request.as_object_mut() else {
        return;
    };

    for field in [
        "context_management",
        "max_completion_tokens",
        "max_output_tokens",
        "max_tokens",
        "metadata",
        "temperature",
        "user",
    ] {
        object.remove(field);
    }
}

fn add_codex_request_fields(request: &mut Value, session_id: Option<&str>) {
    strip_unsupported_codex_fields(request);

    let Some(object) = request.as_object_mut() else {
        return;
    };

    object.insert("store".into(), Value::Bool(false));
    object.insert("parallel_tool_calls".into(), Value::Bool(true));
    object.insert("tool_choice".into(), Value::String("auto".into()));
    object.insert(
        "include".into(),
        Value::Array(vec![Value::String("reasoning.encrypted_content".into())]),
    );
    object.insert(
        "text".into(),
        serde_json::json!({
            "verbosity": "medium",
        }),
    );
    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        object.insert(
            "prompt_cache_key".into(),
            Value::String(session_id.to_string()),
        );
    }
}

fn build_headers(
    account_id: &str,
    api_key: &str,
    session_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("authorization".to_string(), format!("Bearer {api_key}")),
        ("chatgpt-account-id".to_string(), account_id.to_string()),
        ("originator".to_string(), "imp".to_string()),
        (
            "OpenAI-Beta".to_string(),
            "responses=experimental".to_string(),
        ),
        ("accept".to_string(), "text/event-stream".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "user-agent".to_string(),
            format!("imp/{}", env!("CARGO_PKG_VERSION")),
        ),
    ];

    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        headers.push(("session_id".to_string(), session_id.to_string()));
    }

    headers
}

#[async_trait]
impl Provider for OpenAiCodexProvider {
    fn stream(
        &self,
        model: &Model,
        context: Context,
        options: RequestOptions,
        api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
        let account_id = match extract_account_id(api_key) {
            Ok(account_id) => account_id,
            Err(error) => {
                return Box::pin(futures::stream::once(async move { Err(error) }));
            }
        };

        let mut request = build_request_json(model, context.clone(), options);
        // The ChatGPT/Codex backend maintains its own server-side context when
        // requests share a session id. imp already sends the full active
        // history for each turn, so attaching a stable session id can make the
        // backend accumulate duplicate context and report a full window while
        // the local active request is still small.
        add_codex_request_fields(&mut request, None);
        let headers = build_headers(&account_id, api_key, None);
        stream_response_json(
            self.client.clone(),
            CODEX_API_URL.to_string(),
            headers,
            request,
        )
    }

    async fn resolve_auth(&self, auth: &AuthStore) -> Result<ApiKey> {
        if let Some(oauth) = auth.get_oauth("openai") {
            return Ok(oauth.access_token.clone());
        }
        if let Some(oauth) = auth.get_oauth("openai-codex") {
            return Ok(oauth.access_token.clone());
        }
        Err(Error::Auth(
            "No ChatGPT OAuth credential found. Run `imp login openai`.".into(),
        ))
    }

    fn id(&self) -> &str {
        "openai-codex"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_request_strips_unsupported_fields() {
        let mut request = serde_json::json!({
            "model": "gpt-5.4",
            "max_output_tokens": 1234,
            "temperature": 0.2,
            "metadata": {"source": "test"},
        });

        add_codex_request_fields(&mut request, None);

        let object = request.as_object().expect("request object");
        assert!(!object.contains_key("max_output_tokens"));
        assert!(!object.contains_key("temperature"));
        assert!(!object.contains_key("metadata"));
        assert_eq!(object.get("store"), Some(&Value::Bool(false)));
        assert_eq!(
            object.get("tool_choice"),
            Some(&Value::String("auto".into()))
        );
    }

    #[test]
    fn codex_request_leaves_max_output_tokens_absent_when_unset() {
        let mut request = serde_json::json!({
            "model": "gpt-5.4"
        });

        add_codex_request_fields(&mut request, None);

        let object = request.as_object().expect("request object");
        assert!(!object.contains_key("max_output_tokens"));
    }
}
