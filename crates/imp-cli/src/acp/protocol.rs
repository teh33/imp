use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum JsonRpcId {
    Number(i64),
    String(String),
    Null,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum JsonRpcOutbound {
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentRequest {
    Initialize(InitializeParams),
    SessionNew(SessionNewParams),
    SessionLoad(SessionLoadParams),
    SessionResume(SessionLoadParams),
    SessionPrompt(SessionPromptParams),
    SessionClose(SessionIdParams),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentNotification {
    SessionCancel(SessionIdParams),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeParams {
    pub protocol_version: u32,
    #[serde(default)]
    pub client_capabilities: ClientCapabilities,
    #[serde(default)]
    pub client_info: Option<ImplementationInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientCapabilities {
    #[serde(default)]
    pub fs: Option<ClientFsCapabilities>,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientFsCapabilities {
    #[serde(default)]
    pub read_text_file: bool,
    #[serde(default)]
    pub write_text_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImplementationInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeResult {
    pub protocol_version: u32,
    pub agent_capabilities: AgentCapabilities,
    pub agent_info: ImplementationInfo,
    pub auth_methods: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentCapabilities {
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub load_session: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_capabilities: Option<PromptCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_capabilities: Option<SessionCapabilities>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptCapabilities {
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub image: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub audio: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub embedded_context: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionNewParams {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionLoadParams {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionIdParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlock {
    Text {
        text: String,
    },
    Resource {
        resource: EmbeddedResource,
    },
    ResourceLink {
        uri: String,
        #[serde(default)]
        name: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmbeddedResource {
    pub uri: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionUpdateParams {
    pub session_id: String,
    pub update: SessionUpdate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub(crate) enum SessionUpdate {
    AgentMessageChunk {
        content: ContentBlock,
    },
    UserMessageChunk {
        content: ContentBlock,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        title: String,
        #[serde(default = "default_tool_kind")]
        kind: ToolKind,
        #[serde(default = "default_tool_status")]
        status: ToolCallStatus,
        #[serde(skip_serializing_if = "Option::is_none", rename = "rawInput")]
        raw_input: Option<Value>,
    },
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ToolCallStatus>,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        content: Vec<ToolCallContent>,
        #[serde(skip_serializing_if = "Option::is_none", rename = "rawOutput")]
        raw_output: Option<Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolCallContent {
    Content { content: ContentBlock },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionPromptResult {
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none", rename = "_meta")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

fn default_tool_kind() -> ToolKind {
    ToolKind::Other
}

fn default_tool_status() -> ToolCallStatus {
    ToolCallStatus::Pending
}

impl JsonRpcResponse {
    pub(crate) fn success(id: JsonRpcId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn error(id: JsonRpcId, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

impl JsonRpcNotification {
    pub(crate) fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

pub(crate) fn parse_message(line: &str) -> Result<JsonRpcMessage, JsonRpcError> {
    let value: Value = serde_json::from_str(line).map_err(|error| JsonRpcError {
        code: -32700,
        message: format!("parse error: {error}"),
    })?;

    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(JsonRpcError {
            code: -32600,
            message: "invalid request: jsonrpc must be \"2.0\"".to_string(),
        });
    }

    if value.get("method").is_some() {
        if value.get("id").is_some() {
            serde_json::from_value(value)
                .map(JsonRpcMessage::Request)
                .map_err(invalid_request)
        } else {
            serde_json::from_value(value)
                .map(JsonRpcMessage::Notification)
                .map_err(invalid_request)
        }
    } else if value.get("result").is_some() || value.get("error").is_some() {
        serde_json::from_value(value)
            .map(JsonRpcMessage::Response)
            .map_err(invalid_request)
    } else {
        Err(JsonRpcError {
            code: -32600,
            message: "invalid request: missing method, result, or error".to_string(),
        })
    }
}

pub(crate) fn parse_agent_request(
    request: JsonRpcRequest,
) -> Result<(JsonRpcId, AgentRequest), JsonRpcResponse> {
    let id = request.id;
    let parsed = match request.method.as_str() {
        "initialize" => parse_params(request.params).map(AgentRequest::Initialize),
        "session/new" => parse_params(request.params).map(AgentRequest::SessionNew),
        "session/load" => parse_params(request.params).map(AgentRequest::SessionLoad),
        "session/resume" => parse_params(request.params).map(AgentRequest::SessionResume),
        "session/prompt" => parse_params(request.params).map(AgentRequest::SessionPrompt),
        "session/close" => parse_params(request.params).map(AgentRequest::SessionClose),
        _ => Err(JsonRpcError {
            code: -32601,
            message: format!("method not found: {}", request.method),
        }),
    };

    parsed
        .map(|request| (id.clone(), request))
        .map_err(|error| JsonRpcResponse::error(id, error.code, error.message))
}

pub(crate) fn parse_agent_notification(
    notification: JsonRpcNotification,
) -> Result<AgentNotification, JsonRpcError> {
    match notification.method.as_str() {
        "session/cancel" => parse_params(notification.params).map(AgentNotification::SessionCancel),
        _ => Err(JsonRpcError {
            code: -32601,
            message: format!("method not found: {}", notification.method),
        }),
    }
}

pub(crate) fn initialize_result(version: impl Into<String>) -> InitializeResult {
    InitializeResult {
        protocol_version: PROTOCOL_VERSION,
        agent_capabilities: AgentCapabilities {
            // Keep the initial protocol surface conservative until durable replay/resume is wired.
            load_session: false,
            prompt_capabilities: Some(PromptCapabilities {
                embedded_context: true,
                ..PromptCapabilities::default()
            }),
            session_capabilities: Some(SessionCapabilities {
                resume: None,
                close: Some(serde_json::json!({})),
            }),
        },
        agent_info: ImplementationInfo {
            name: "imp".to_string(),
            title: Some("imp".to_string()),
            version: Some(version.into()),
        },
        auth_methods: Vec::new(),
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, JsonRpcError> {
    serde_json::from_value(params).map_err(|error| JsonRpcError {
        code: -32602,
        message: format!("invalid params: {error}"),
    })
}

fn invalid_request(error: serde_json::Error) -> JsonRpcError {
    JsonRpcError {
        code: -32600,
        message: format!("invalid request: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_initialize_request() {
        let line = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true}},"clientInfo":{"name":"zed","title":"Zed","version":"1.0"}}}"#;
        let JsonRpcMessage::Request(request) = parse_message(line).unwrap() else {
            panic!("expected request");
        };
        let (id, AgentRequest::Initialize(params)) = parse_agent_request(request).unwrap() else {
            panic!("expected initialize");
        };

        assert_eq!(id, JsonRpcId::Number(0));
        assert_eq!(params.protocol_version, 1);
        assert_eq!(params.client_info.unwrap().name, "zed");
        assert_eq!(params.client_capabilities.fs.unwrap().read_text_file, true);
    }

    #[test]
    fn serializes_initialize_result_with_truthful_capabilities() {
        let result = serde_json::to_value(initialize_result("test-version")).unwrap();

        assert_eq!(result["protocolVersion"], 1);
        assert_eq!(result["agentInfo"]["name"], "imp");
        assert_eq!(result["agentCapabilities"]["loadSession"], Value::Null);
        assert_eq!(
            result["agentCapabilities"]["promptCapabilities"]["embeddedContext"],
            true
        );
        assert_eq!(
            result["agentCapabilities"]["sessionCapabilities"]["close"],
            json!({})
        );
    }

    #[test]
    fn parses_session_prompt_content_blocks() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::String("p1".to_string()),
            method: "session/prompt".to_string(),
            params: json!({
                "sessionId": "sess_1",
                "prompt": [
                    {"type": "text", "text": "hello"},
                    {"type": "resource", "resource": {"uri": "file:///tmp/a.rs", "mimeType": "text/rust", "text": "fn main() {}"}},
                    {"type": "resource_link", "uri": "file:///tmp/b.rs", "name": "b.rs"}
                ]
            }),
        };

        let (_, AgentRequest::SessionPrompt(params)) = parse_agent_request(request).unwrap() else {
            panic!("expected prompt");
        };

        assert_eq!(params.session_id, "sess_1");
        assert_eq!(params.prompt.len(), 3);
        assert!(matches!(params.prompt[0], ContentBlock::Text { .. }));
        assert!(matches!(params.prompt[1], ContentBlock::Resource { .. }));
        assert!(matches!(
            params.prompt[2],
            ContentBlock::ResourceLink { .. }
        ));
    }

    #[test]
    fn parses_cancel_notification() {
        let line = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"sess_1"}}"#;
        let JsonRpcMessage::Notification(notification) = parse_message(line).unwrap() else {
            panic!("expected notification");
        };
        let AgentNotification::SessionCancel(params) =
            parse_agent_notification(notification).unwrap();

        assert_eq!(params.session_id, "sess_1");
    }

    #[test]
    fn unknown_request_method_returns_method_not_found_response() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(9),
            method: "bogus".to_string(),
            params: json!({}),
        };

        let response = parse_agent_request(request).unwrap_err();
        let error = response.error.unwrap();
        assert_eq!(response.id, JsonRpcId::Number(9));
        assert_eq!(error.code, -32601);
        assert!(error.message.contains("bogus"));
    }

    #[test]
    fn invalid_params_return_json_rpc_invalid_params() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(2),
            method: "session/new".to_string(),
            params: json!({"cwd": 42}),
        };

        let response = parse_agent_request(request).unwrap_err();
        assert_eq!(response.error.unwrap().code, -32602);
    }

    #[test]
    fn serializes_session_update_tool_call() {
        let params = SessionUpdateParams {
            session_id: "sess".to_string(),
            update: SessionUpdate::ToolCall {
                tool_call_id: "call_1".to_string(),
                title: "Run tests".to_string(),
                kind: ToolKind::Execute,
                status: ToolCallStatus::InProgress,
                raw_input: Some(json!({"command": "cargo test"})),
            },
        };

        let value = serde_json::to_value(params).unwrap();
        assert_eq!(value["sessionId"], "sess");
        assert_eq!(value["update"]["sessionUpdate"], "tool_call");
        assert_eq!(value["update"]["toolCallId"], "call_1");
        assert_eq!(value["update"]["kind"], "execute");
        assert_eq!(value["update"]["status"], "in_progress");
    }

    #[test]
    fn serializes_prompt_result_stop_reason() {
        let result = SessionPromptResult {
            stop_reason: StopReason::Cancelled,
            meta: None,
        };

        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["stopReason"], "cancelled");
    }
}
