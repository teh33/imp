use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::auth::OAuthCredential;
use crate::error::{Error, Result};

use super::pkce::PkceChallenge;

// Anthropic OAuth constants (CLIENT_ID decoded from base64: OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl)
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const MANUAL_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const REDIRECT_URI: &str = "http://localhost:53692/callback";
const SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:file_upload";

const SUCCESS_HTML: &str = "\
<html><body style=\"font-family:system-ui;text-align:center;padding:60px\">\
<h1>&#10003; Logged in</h1>\
<p>You can close this window and return to imp.</p>\
</body></html>";

const ERROR_HTML: &str = "\
<html><body style=\"font-family:system-ui;text-align:center;padding:60px\">\
<h1>Error</h1><p>OAuth state mismatch. Please try again.</p>\
</body></html>";

/// Token endpoint response.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
}

/// Anthropic Max/Pro subscription OAuth handler.
pub struct AnthropicOAuth {
    client_id: String,
    token_url: String,
}

impl Default for AnthropicOAuth {
    fn default() -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            token_url: TOKEN_URL.to_string(),
        }
    }
}

impl AnthropicOAuth {
    /// Create with default Anthropic endpoints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with a custom token URL (for testing with a mock server).
    pub fn with_token_url(token_url: String) -> Self {
        Self {
            client_id: CLIENT_ID.to_string(),
            token_url,
        }
    }

    /// Build the authorization URL the user should visit.
    pub fn build_authorize_url(&self, pkce: &PkceChallenge) -> String {
        let mut url = Url::parse(AUTHORIZE_URL).expect("valid authorize URL constant");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &pkce.verifier)
            .append_pair("code", "true");
        url.to_string()
    }

    /// Exchange an authorization code for access + refresh tokens.
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<OAuthCredential> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.client_id.as_str()),
                ("code", code),
                ("state", verifier),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body =
                crate::auth::redact_oauth_error_body(&response.text().await.unwrap_or_default());
            return Err(Error::Auth(format!(
                "Token exchange failed ({status}): {body}"
            )));
        }

        let token: TokenResponse = response.json().await?;
        let expires_at = crate::now() + token.expires_in.saturating_sub(300);

        Ok(OAuthCredential {
            access_token: token.access_token,
            refresh_token: token.refresh_token.unwrap_or_default(),
            expires_at,
        })
    }

    /// Refresh an expired OAuth token.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthCredential> {
        let client = reqwest::Client::new();
        let response = client
            .post(&self.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("scope", SCOPES),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body =
                crate::auth::redact_oauth_error_body(&response.text().await.unwrap_or_default());
            return Err(Error::Auth(format!(
                "Token refresh failed ({status}): {body}"
            )));
        }

        let token: TokenResponse = response.json().await?;
        let expires_at = crate::now() + token.expires_in.saturating_sub(300);

        Ok(OAuthCredential {
            access_token: token.access_token,
            refresh_token: token
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_string()),
            expires_at,
        })
    }

    /// Full login flow: generate PKCE, start callback server, open browser, exchange code.
    ///
    /// `open_url` is called with the authorization URL to open in the browser.
    /// `manual_code_input` is called if the callback server times out, to get manual input.
    pub async fn login<F, G, Fut>(
        &self,
        open_url: F,
        manual_code_input: G,
    ) -> Result<OAuthCredential>
    where
        F: FnOnce(&str),
        G: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<String>>,
    {
        let pkce = PkceChallenge::generate();
        let auth_url = self.build_authorize_url(&pkce);

        let server = CallbackServer::bind(CALLBACK_HOST, CALLBACK_PORT).await?;
        open_url(&auth_url);

        // Wait for callback with 5 minute timeout
        let timeout = Duration::from_secs(300);
        match server.wait_for_code(&pkce.verifier, timeout).await {
            Ok(code) => {
                self.exchange_code(&code, &pkce.verifier, REDIRECT_URI)
                    .await
            }
            Err(_) => {
                // Callback failed — try manual input
                let input = manual_code_input()
                    .await
                    .ok_or_else(|| Error::Auth("Login cancelled".into()))?;
                let code = parse_manual_code(&input)?;
                self.exchange_code(&code, &pkce.verifier, MANUAL_REDIRECT_URI)
                    .await
            }
        }
    }
}

/// Parse an authorization code from user input.
///
/// Accepts:
/// - A raw code string
/// - A full URL with a `code` query parameter
/// - A URL with multiple query parameters
pub fn parse_manual_code(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::Auth(
            "Empty input — expected an authorization code or redirect URL".into(),
        ));
    }

    // Try parsing as a URL with a code parameter
    if let Ok(url) = Url::parse(input) {
        if let Some(code) = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
        {
            if !code.is_empty() {
                return Ok(code);
            }
        }
    }

    // Accept as raw code if it looks reasonable (no whitespace, not empty)
    if !input.contains(char::is_whitespace) {
        return Ok(input.to_string());
    }

    Err(Error::Auth(
        "Could not parse authorization code from input".into(),
    ))
}

/// Local HTTP server that listens for the OAuth callback redirect.
pub struct CallbackServer {
    listener: TcpListener,
    /// The port the server is bound to.
    pub port: u16,
}

impl CallbackServer {
    /// Bind the callback server to the given host and port.
    /// Use port 0 to let the OS assign a random available port.
    pub async fn bind(host: &str, port: u16) -> Result<Self> {
        let listener = TcpListener::bind(format!("{host}:{port}")).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// Wait for an OAuth callback request with the given state.
    ///
    /// Returns the authorization code on success, or an error on timeout/mismatch.
    pub async fn wait_for_code(self, expected_state: &str, timeout: Duration) -> Result<String> {
        let accept = tokio::time::timeout(timeout, self.listener.accept());
        let (mut stream, _) = accept
            .await
            .map_err(|_| Error::Auth("Timeout waiting for OAuth callback".into()))?
            .map_err(|e| Error::Auth(format!("Failed to accept callback connection: {e}")))?;

        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // Parse "GET /callback?code=xxx&state=yyy HTTP/1.1"
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| Error::Auth("Invalid HTTP request in callback".into()))?;

        // Only handle the callback path
        if !path.starts_with(CALLBACK_PATH) {
            let _ = send_response(&mut stream, 404, "Not Found").await;
            return Err(Error::Auth(format!("Unexpected path: {path}")));
        }

        let url = Url::parse(&format!("http://localhost{path}"))
            .map_err(|e| Error::Auth(format!("Failed to parse callback URL: {e}")))?;

        let params: HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let code = params
            .get("code")
            .filter(|c| !c.is_empty())
            .ok_or_else(|| Error::Auth("No authorization code in callback".into()))?;

        let state = params
            .get("state")
            .ok_or_else(|| Error::Auth("No state parameter in callback".into()))?;

        if state != expected_state {
            let _ = send_response(&mut stream, 400, ERROR_HTML).await;
            return Err(Error::Auth("State mismatch in OAuth callback".into()));
        }

        send_response(&mut stream, 200, SUCCESS_HTML).await?;
        Ok(code.clone())
    }
}

async fn send_response(stream: &mut tokio::net::TcpStream, status: u16, body: &str) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener as TokioListener;

    // --- Authorization URL ---

    #[tokio::test]
    async fn test_oauth_build_authorize_url() {
        let oauth = AnthropicOAuth::new();
        let pkce = PkceChallenge::generate();
        let url_str = oauth.build_authorize_url(&pkce);
        let url = Url::parse(&url_str).expect("valid URL");

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("claude.ai"));
        assert_eq!(url.path(), "/oauth/authorize");

        let params: HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        assert_eq!(params.get("client_id").unwrap(), CLIENT_ID);
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("redirect_uri").unwrap(), REDIRECT_URI);
        assert_eq!(params.get("scope").unwrap(), SCOPES);
        assert_eq!(params.get("code_challenge").unwrap(), &pkce.challenge);
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("state").unwrap(), &pkce.verifier);
    }

    // --- Manual code parsing ---

    #[test]
    fn test_oauth_parse_manual_code_raw() {
        let code = parse_manual_code("abc123def456").unwrap();
        assert_eq!(code, "abc123def456");
    }

    #[test]
    fn test_oauth_parse_manual_code_url() {
        let input = "https://platform.claude.com/oauth/code/callback?code=mycode123";
        let code = parse_manual_code(input).unwrap();
        assert_eq!(code, "mycode123");
    }

    #[test]
    fn test_oauth_parse_manual_code_url_with_extra_params() {
        let input =
            "https://platform.claude.com/oauth/code/callback?code=mycode123&state=somestate&foo=bar";
        let code = parse_manual_code(input).unwrap();
        assert_eq!(code, "mycode123");
    }

    #[test]
    fn test_oauth_parse_manual_code_localhost_url() {
        let input = "http://localhost:53692/callback?code=localcode&state=verifier123";
        let code = parse_manual_code(input).unwrap();
        assert_eq!(code, "localcode");
    }

    #[test]
    fn test_oauth_parse_manual_code_empty_fails() {
        assert!(parse_manual_code("").is_err());
        assert!(parse_manual_code("  ").is_err());
    }

    #[test]
    fn test_oauth_parse_manual_code_whitespace_fails() {
        assert!(parse_manual_code("has spaces in it").is_err());
    }

    // --- Callback server ---

    #[tokio::test]
    async fn test_oauth_callback_server_receives_code() {
        let server = CallbackServer::bind("127.0.0.1", 0).await.unwrap();
        let port = server.port;
        let expected_state = "test-verifier-state";

        let server_handle = tokio::spawn(async move {
            server
                .wait_for_code(expected_state, Duration::from_secs(5))
                .await
        });

        // Give server a moment to start listening
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Simulate browser callback
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let request = format!(
            "GET /callback?code=auth-code-123&state={expected_state} HTTP/1.1\r\n\
             Host: localhost:{port}\r\n\
             \r\n"
        );
        client.write_all(request.as_bytes()).await.unwrap();

        // Read response
        let mut response = vec![0u8; 4096];
        let n = client.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);
        assert!(response_str.contains("200 OK"));
        assert!(response_str.contains("Logged in"));

        let code = server_handle.await.unwrap().unwrap();
        assert_eq!(code, "auth-code-123");
    }

    #[tokio::test]
    async fn test_oauth_callback_server_invalid_state() {
        let server = CallbackServer::bind("127.0.0.1", 0).await.unwrap();
        let port = server.port;

        let server_handle = tokio::spawn(async move {
            server
                .wait_for_code("expected-state", Duration::from_secs(5))
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let request = "GET /callback?code=some-code&state=wrong-state HTTP/1.1\r\n\
                        Host: localhost\r\n\r\n";
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 4096];
        let n = client.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);
        assert!(response_str.contains("400"));

        let result = server_handle.await.unwrap();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("State mismatch"));
    }

    #[tokio::test]
    async fn test_oauth_callback_server_timeout() {
        let server = CallbackServer::bind("127.0.0.1", 0).await.unwrap();
        let result = server
            .wait_for_code("state", Duration::from_millis(100))
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Timeout"));
    }

    // --- Mock token server helper ---

    async fn start_mock_token_server(_response_json: &str) -> (TokioListener, u16) {
        let listener = TokioListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    async fn serve_token_response(listener: TokioListener, status: u16, body: String) {
        let (mut stream, _) = listener.accept().await.unwrap();

        // Read request (consume it)
        let mut buf = vec![0u8; 8192];
        let _ = stream.read(&mut buf).await.unwrap();

        let status_text = if status == 200 { "OK" } else { "Bad Request" };
        let response = format!(
            "HTTP/1.1 {status} {status_text}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    }

    // --- Token exchange ---

    #[tokio::test]
    async fn test_oauth_exchange_code() {
        let body = serde_json::json!({
            "access_token": "sk-ant-test-access-token",
            "refresh_token": "rt-test-refresh-token",
            "expires_in": 3600
        })
        .to_string();

        let (listener, port) = start_mock_token_server(&body).await;

        tokio::spawn(serve_token_response(listener, 200, body));

        let oauth = AnthropicOAuth::with_token_url(format!("http://127.0.0.1:{port}/token"));
        let cred = oauth
            .exchange_code("auth-code-123", "test-verifier", REDIRECT_URI)
            .await
            .unwrap();

        assert_eq!(cred.access_token, "sk-ant-test-access-token");
        assert_eq!(cred.refresh_token, "rt-test-refresh-token");
        // expires_at should be roughly now + 3600 - 300 = now + 3300
        let expected_min = crate::now() + 3200;
        let expected_max = crate::now() + 3400;
        assert!(
            cred.expires_at >= expected_min && cred.expires_at <= expected_max,
            "expires_at {} not in range [{}, {}]",
            cred.expires_at,
            expected_min,
            expected_max
        );
    }

    #[tokio::test]
    async fn test_oauth_exchange_code_failure() {
        let body = r#"{"error": "invalid_grant", "error_description": "Code expired"}"#.to_string();
        let (listener, port) = start_mock_token_server(&body).await;

        tokio::spawn(serve_token_response(listener, 400, body));

        let oauth = AnthropicOAuth::with_token_url(format!("http://127.0.0.1:{port}/token"));
        let result = oauth
            .exchange_code("bad-code", "test-verifier", REDIRECT_URI)
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Token exchange failed"));
        assert!(err_msg.contains("400"));
    }

    // --- Token refresh ---

    #[tokio::test]
    async fn test_oauth_refresh_token() {
        let body = serde_json::json!({
            "access_token": "sk-ant-new-access-token",
            "refresh_token": "rt-new-refresh-token",
            "expires_in": 7200
        })
        .to_string();

        let (listener, port) = start_mock_token_server(&body).await;

        tokio::spawn(serve_token_response(listener, 200, body));

        let oauth = AnthropicOAuth::with_token_url(format!("http://127.0.0.1:{port}/token"));
        let cred = oauth.refresh_token("rt-old-refresh-token").await.unwrap();

        assert_eq!(cred.access_token, "sk-ant-new-access-token");
        assert_eq!(cred.refresh_token, "rt-new-refresh-token");
        let expected_min = crate::now() + 6800;
        let expected_max = crate::now() + 7000;
        assert!(
            cred.expires_at >= expected_min && cred.expires_at <= expected_max,
            "expires_at {} not in range [{}, {}]",
            cred.expires_at,
            expected_min,
            expected_max
        );
    }

    #[tokio::test]
    async fn test_oauth_refresh_token_keeps_old_refresh_when_none_returned() {
        // Some providers don't return a new refresh token
        let body = serde_json::json!({
            "access_token": "sk-ant-refreshed",
            "expires_in": 3600
        })
        .to_string();

        let (listener, port) = start_mock_token_server(&body).await;

        tokio::spawn(serve_token_response(listener, 200, body));

        let oauth = AnthropicOAuth::with_token_url(format!("http://127.0.0.1:{port}/token"));
        let cred = oauth.refresh_token("rt-original-token").await.unwrap();

        assert_eq!(cred.access_token, "sk-ant-refreshed");
        // Should keep the old refresh token when none returned
        assert_eq!(cred.refresh_token, "rt-original-token");
    }

    #[tokio::test]
    async fn test_oauth_refresh_token_failure() {
        let body = r#"{"error": "invalid_grant", "error_description": "Refresh token revoked"}"#
            .to_string();
        let (listener, port) = start_mock_token_server(&body).await;

        tokio::spawn(serve_token_response(listener, 401, body));

        let oauth = AnthropicOAuth::with_token_url(format!("http://127.0.0.1:{port}/token"));
        let result = oauth.refresh_token("rt-revoked").await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Token refresh failed"));
    }
}
