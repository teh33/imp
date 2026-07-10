mod action;
mod approval;
mod client;
mod config;
mod diagnostics;
mod session;

use std::sync::Arc;
use std::time::Duration;

use action::BrowserAction;
use approval::{request_browser_approval, BrowserApprovalStore};
use async_trait::async_trait;
pub use config::BrowserConfig;
pub use diagnostics::{
    diagnose_browser, resolve_lightpanda_binary, BrowserDiagnostic, DiagnosticCheck,
    DiagnosticStatus,
};
use serde_json::{json, Value};
use session::BrowserSessionManager;
use tokio::sync::Mutex;

use super::{Tool, ToolContext, ToolOutput};
use crate::error::Result;
use crate::reference_monitor::{ResourceScope, ToolMetadata};

const MAX_TOOL_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BrowserSessionId(String);

impl BrowserSessionId {
    fn new() -> Self {
        Self(format!("browser_{}", uuid::Uuid::new_v4().simple()))
    }

    pub(crate) fn parse(value: &str) -> std::result::Result<Self, String> {
        if !value.starts_with("browser_")
            || value.len() != 40
            || !value[8..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err("invalid browser session_id".into());
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct BrowserTool {
    config: BrowserConfig,
    sessions: Arc<Mutex<BrowserSessionManager>>,
    approvals: Arc<Mutex<BrowserApprovalStore>>,
}

impl BrowserTool {
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            config,
            sessions: Arc::new(Mutex::new(BrowserSessionManager::new())),
            approvals: Arc::new(Mutex::new(BrowserApprovalStore::default())),
        }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn label(&self) -> &str {
        "Browser"
    }

    fn description(&self) -> &str {
        "Use a stateful Lightpanda browser for JavaScript pages. Start a session, navigate, observe semantic elements, extract content, interact, then stop it. Lightpanda is headless and does not provide screenshots."
    }

    fn parameters(&self) -> Value {
        browser_schema()
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn is_readonly_call(&self, params: &Value) -> bool {
        BrowserAction::parse(params).is_ok_and(BrowserAction::is_readonly)
    }

    fn policy_metadata_for(&self, params: &Value) -> ToolMetadata {
        let action = BrowserAction::parse(params).unwrap_or(BrowserAction::Click);
        let readonly = action.is_readonly();
        let mut metadata = ToolMetadata::new(self.name(), action.policy_kind());
        metadata.readonly = readonly;
        metadata.network = !matches!(action, BrowserAction::Start | BrowserAction::Stop);
        metadata.external_side_effect = matches!(
            action,
            BrowserAction::Click
                | BrowserAction::Fill
                | BrowserAction::Press
                | BrowserAction::Select
                | BrowserAction::Check
                | BrowserAction::Scroll
        );
        metadata.default_requires_approval = false;
        metadata.requires_approval = action.requires_fresh_approval(params);
        if let Some(url) = params.get("url").and_then(Value::as_str) {
            metadata.resource_scopes.push(ResourceScope::Network {
                host: url::Url::parse(url)
                    .ok()
                    .and_then(|parsed| parsed.host_str().map(str::to_string)),
            });
        }
        metadata
    }

    async fn request_approval(
        &self,
        params: &Value,
        ui: Arc<dyn crate::ui::UserInterface>,
    ) -> crate::tools::ToolApproval {
        let action = match BrowserAction::parse(params) {
            Ok(action) => action,
            Err(error) => return crate::tools::ToolApproval::denied(error),
        };
        if let Err(error) = action.validate(params) {
            return crate::tools::ToolApproval::denied(error);
        }
        let session_id = match session_id(params) {
            Ok(session_id) => session_id,
            Err(error) => return crate::tools::ToolApproval::denied(error),
        };
        let mut approval_params = params.clone();
        let sessions = self.sessions.lock().await;
        if !sessions.contains(&session_id) {
            return crate::tools::ToolApproval::denied("browser session was not found");
        }
        if let Some(domain) = sessions.domain(&session_id) {
            if let Some(params) = approval_params.as_object_mut() {
                params.insert("approval_domain".into(), Value::String(domain));
            }
        }
        drop(sessions);
        request_browser_approval(&self.approvals, &approval_params, ui).await
    }

    async fn execute(&self, _call_id: &str, params: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let action = match BrowserAction::parse(&params) {
            Ok(action) => action,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if let Err(error) = action.validate(&params) {
            return Ok(ToolOutput::error(error));
        }
        if ctx.is_cancelled() {
            return Ok(ToolOutput::error("browser action cancelled"));
        }
        if action == BrowserAction::Start {
            return self.start(&ctx).await;
        }
        let session_id = match session_id(&params) {
            Ok(id) => id,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if action == BrowserAction::Stop {
            return self.stop(&session_id).await;
        }
        let call = action.mcp_call(&params, self.config.timeout_ms);
        let timeout_ms = params
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(self.config.timeout_ms)
            .clamp(100, 120_000);
        let output = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .call(
                    &session_id,
                    call.tool,
                    call.arguments,
                    Arc::clone(&ctx.cancelled),
                    Duration::from_millis(timeout_ms),
                )
                .await
        };
        let (domain, sequence) = {
            let sessions = self.sessions.lock().await;
            (sessions.domain(&session_id), sessions.sequence(&session_id))
        };
        match output {
            Ok(output) => {
                if action == BrowserAction::Navigate {
                    let domain = navigated_domain(&output.text);
                    self.sessions
                        .lock()
                        .await
                        .set_domain(&session_id, domain.clone());
                    return Ok(tool_output(
                        action,
                        &session_id,
                        call.tool,
                        output,
                        domain,
                        sequence,
                    ));
                }
                Ok(tool_output(
                    action,
                    &session_id,
                    call.tool,
                    output,
                    domain,
                    sequence,
                ))
            }
            Err(error) => Ok(ToolOutput::error(error)),
        }
    }
}

impl BrowserTool {
    async fn start(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        if let Err(error) = self.config.validate() {
            return Ok(ToolOutput::error(error));
        }
        let cancelled = Arc::clone(&ctx.cancelled);
        let result = tokio::select! {
            result = async {
                self.sessions
                    .lock()
                    .await
                    .start(&self.config, &ctx.cwd)
                    .await
            } => result,
            _ = wait_for_cancellation(cancelled) => Err("browser start cancelled".into()),
        };
        match result {
            Ok(id) => Ok(ToolOutput {
                content: vec![imp_llm::ContentBlock::Text {
                    text: format!(
                        "Started Lightpanda browser session `{}`. Use this session_id for subsequent browser actions.",
                        id.as_str()
                    ),
                }],
                details: json!({
                    "action": "start",
                    "engine": "lightpanda",
                    "session_id": id.as_str(),
                    "sequence": 0,
                    "capabilities": {"semantic": true, "screenshots": false}
                }),
                is_error: false,
            }),
            Err(error) => Ok(ToolOutput::error(error)),
        }
    }

    async fn stop(&self, id: &BrowserSessionId) -> Result<ToolOutput> {
        self.approvals.lock().await.remove_session(id);
        match self.sessions.lock().await.stop(id).await {
            Ok(()) => Ok(ToolOutput {
                content: vec![imp_llm::ContentBlock::Text {
                    text: format!("Stopped browser session `{}`.", id.as_str()),
                }],
                details: json!({"action": "stop", "session_id": id.as_str(), "sequence": null}),
                is_error: false,
            }),
            Err(error) => Ok(ToolOutput::error(error)),
        }
    }
}

async fn wait_for_cancellation(cancelled: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;

    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn session_id(params: &Value) -> std::result::Result<BrowserSessionId, String> {
    let value = params
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "browser action requires session_id".to_string())?;
    BrowserSessionId::parse(value)
}

fn navigated_domain(output: &str) -> Option<String> {
    let url = output.lines().find_map(|line| line.strip_prefix("URL: "))?;
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|host| host.to_ascii_lowercase())
}

fn tool_output(
    action: BrowserAction,
    id: &BrowserSessionId,
    backend_tool: &str,
    output: client::McpOutput,
    domain: Option<String>,
    sequence: Option<u64>,
) -> ToolOutput {
    let mut text = output.text;
    let total_bytes = text.len();
    if text.len() > MAX_TOOL_OUTPUT_BYTES {
        let mut boundary = MAX_TOOL_OUTPUT_BYTES;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n[truncated]");
    }
    let interactive_elements = (action == BrowserAction::Observe)
        .then(|| text.lines().filter(|line| !line.trim().is_empty()).count());
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": format!("{action:?}").to_lowercase(),
            "engine": "lightpanda",
            "session_id": id.as_str(),
            "backend_tool": backend_tool,
            "total_bytes": total_bytes,
            "domain": domain,
            "sequence": sequence,
            "interactive_elements": interactive_elements,
            "truncated": total_bytes > MAX_TOOL_OUTPUT_BYTES
        }),
        is_error: output.is_error,
    }
}

fn browser_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {"type": "string", "enum": ["start", "stop", "navigate", "observe", "markdown", "links", "structured_data", "forms", "extract", "click", "fill", "press", "select", "check", "scroll", "wait", "get_url", "console"]},
            "session_id": {"type": "string", "description": "Session returned by start."},
            "url": {"type": "string", "pattern": "^https?://"},
            "selector": {"type": "string"},
            "backend_node_id": {"type": "integer", "minimum": 1},
            "value": {"type": "string"},
            "key": {"type": "string"},
            "checked": {"type": "boolean"},
            "x": {"type": "integer"},
            "y": {"type": "integer"},
            "schema": {"type": "string", "description": "Lightpanda extraction schema encoded as JSON."},
            "max_bytes": {"type": "integer", "minimum": 1, "maximum": 262144},
            "timeout_ms": {"type": "integer", "minimum": 100, "maximum": 120000}
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod reliability_tests;
#[cfg(test)]
mod tests;
