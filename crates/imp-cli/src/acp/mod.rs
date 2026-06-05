pub(crate) mod events;
pub(crate) mod protocol;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use imp_core::session::{SessionEntry, SessionManager};
use imp_llm::Message;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;

use self::events::{
    message_to_session_updates, prompt_blocks_to_text, stop_reason_from_agent_status,
};
use self::protocol::{
    initialize_result, parse_agent_notification, parse_agent_request, parse_message,
    AgentNotification, AgentRequest, JsonRpcId, JsonRpcMessage, JsonRpcNotification,
    JsonRpcOutbound, JsonRpcResponse, SessionNewParams, SessionPromptParams, SessionPromptResult,
    SessionUpdateParams,
};

pub(crate) async fn run_stdio_server(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    run_stdio_server_with_io(
        BufReader::new(tokio::io::stdin()),
        BufWriter::new(tokio::io::stdout()),
        version,
    )
    .await
}

pub(crate) async fn run_stdio_server_with_io<R, W>(
    reader: R,
    writer: W,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let (outbound_tx, mut outbound_rx) = mpsc::channel(128);
    let mut server = AcpServer::new(version.to_string(), outbound_tx.clone());
    let mut lines = BufReader::new(reader).lines();
    let mut writer = BufWriter::new(writer);

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        server.handle_line(&line).await;
        drain_outbound(&mut outbound_rx, &mut writer).await?;
    }

    drop(server);
    drop(outbound_tx);
    drain_outbound(&mut outbound_rx, &mut writer).await?;
    Ok(())
}

async fn drain_outbound<W>(
    outbound_rx: &mut mpsc::Receiver<JsonRpcOutbound>,
    writer: &mut BufWriter<W>,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    while let Ok(message) = outbound_rx.try_recv() {
        let encoded = serde_json::to_vec(&message)?;
        writer.write_all(&encoded).await?;
        writer.write_all(b"\n").await?;
    }
    writer.flush().await?;
    Ok(())
}

#[derive(Debug)]
struct AcpServer {
    version: String,
    initialized: bool,
    sessions: HashMap<String, AcpSession>,
    outbound_tx: mpsc::Sender<JsonRpcOutbound>,
}

#[derive(Debug, Clone)]
struct AcpSession {
    cwd: PathBuf,
    path: PathBuf,
    session: SessionManager,
    cancelled: bool,
    prompt_count: u64,
}

impl AcpServer {
    fn new(version: String, outbound_tx: mpsc::Sender<JsonRpcOutbound>) -> Self {
        Self {
            version,
            initialized: false,
            sessions: HashMap::new(),
            outbound_tx,
        }
    }

    async fn handle_line(&mut self, line: &str) {
        match parse_message(line) {
            Ok(JsonRpcMessage::Request(request)) => match parse_agent_request(request) {
                Ok((id, request)) => self.handle_request(id, request).await,
                Err(response) => self.send_response(response).await,
            },
            Ok(JsonRpcMessage::Notification(notification)) => {
                if let Ok(notification) = parse_agent_notification(notification) {
                    self.handle_notification(notification);
                }
            }
            Ok(JsonRpcMessage::Response(_response)) => {}
            Err(error) => {
                self.send_response(JsonRpcResponse::error(
                    JsonRpcId::Null,
                    error.code,
                    error.message,
                ))
                .await;
            }
        }
    }

    async fn handle_request(&mut self, id: JsonRpcId, request: AgentRequest) {
        let response = match request {
            AgentRequest::Initialize(_params) => {
                self.initialized = true;
                JsonRpcResponse::success(id, json!(initialize_result(&self.version)))
            }
            other if !self.initialized => JsonRpcResponse::error(
                id,
                ERROR_NOT_INITIALIZED,
                format!("initialize must be called before {}", method_name(&other)),
            ),
            AgentRequest::SessionNew(params) => self.handle_session_new(id, params),
            AgentRequest::SessionClose(params) => {
                self.sessions.remove(&params.session_id);
                JsonRpcResponse::success(id, json!({}))
            }
            AgentRequest::SessionLoad(params) => self.handle_session_load(id, params, true).await,
            AgentRequest::SessionResume(params) => {
                self.handle_session_load(id, params, false).await
            }
            AgentRequest::SessionPrompt(params) => self.handle_session_prompt(id, params).await,
        };
        self.send_response(response).await;
    }

    async fn send_response(&self, response: JsonRpcResponse) {
        let _ = self
            .outbound_tx
            .send(JsonRpcOutbound::Response(response))
            .await;
    }

    async fn send_notification(&self, method: &str, params: serde_json::Value) {
        let _ = self
            .outbound_tx
            .send(JsonRpcOutbound::Notification(JsonRpcNotification::new(
                method, params,
            )))
            .await;
    }

    fn handle_notification(&mut self, notification: AgentNotification) {
        match notification {
            AgentNotification::SessionCancel(params) => {
                if let Some(session) = self.sessions.get_mut(&params.session_id) {
                    session.cancelled = true;
                }
            }
        }
    }

    fn handle_session_new(&mut self, id: JsonRpcId, params: SessionNewParams) -> JsonRpcResponse {
        let cwd = PathBuf::from(&params.cwd);
        if !cwd.is_absolute() {
            return JsonRpcResponse::error(id, -32602, "session/new cwd must be an absolute path");
        }
        if !params.mcp_servers.is_empty() {
            return JsonRpcResponse::error(
                id,
                ERROR_UNSUPPORTED,
                "ACP MCP server configuration is not supported by imp yet",
            );
        }

        match create_session(&cwd) {
            Ok((session_id, path, session)) => {
                self.sessions.insert(
                    session_id.clone(),
                    AcpSession {
                        cwd,
                        path,
                        session,
                        cancelled: false,
                        prompt_count: 0,
                    },
                );
                JsonRpcResponse::success(id, json!({ "sessionId": session_id }))
            }
            Err(error) => JsonRpcResponse::error(
                id,
                ERROR_SESSION_IO,
                format!("failed to create imp session for ACP: {error}"),
            ),
        }
    }

    async fn handle_session_load(
        &mut self,
        id: JsonRpcId,
        params: self::protocol::SessionLoadParams,
        replay_history: bool,
    ) -> JsonRpcResponse {
        let cwd = PathBuf::from(&params.cwd);
        if !cwd.is_absolute() {
            return JsonRpcResponse::error(
                id,
                -32602,
                "session load/resume cwd must be an absolute path",
            );
        }
        if !params.mcp_servers.is_empty() {
            return JsonRpcResponse::error(
                id,
                ERROR_UNSUPPORTED,
                "ACP MCP server configuration is not supported by imp yet",
            );
        }

        let session_id = params.session_id;
        match load_session_by_id(&session_id) {
            Ok(session) => {
                let Some(path) = session.path().map(Path::to_path_buf) else {
                    return JsonRpcResponse::error(
                        id,
                        ERROR_SESSION_IO,
                        "loaded ACP session did not have a persisted path",
                    );
                };
                if replay_history {
                    for message in session.get_active_messages() {
                        for update in message_to_session_updates(&message, true) {
                            self.send_notification(
                                "session/update",
                                json!(SessionUpdateParams {
                                    session_id: session_id.clone(),
                                    update,
                                }),
                            )
                            .await;
                        }
                    }
                }
                self.sessions.insert(
                    session_id.clone(),
                    AcpSession {
                        cwd,
                        path,
                        session,
                        cancelled: false,
                        prompt_count: 0,
                    },
                );
                JsonRpcResponse::success(id, json!({ "sessionId": session_id }))
            }
            Err(error) => JsonRpcResponse::error(
                id,
                ERROR_SESSION_IO,
                format!("failed to load imp session for ACP: {error}"),
            ),
        }
    }

    async fn handle_session_prompt(
        &mut self,
        id: JsonRpcId,
        params: SessionPromptParams,
    ) -> JsonRpcResponse {
        match prompt_blocks_to_text(&params.prompt) {
            Ok(prompt) => {
                let (cancelled, prompt_count) = {
                    let Some(session) = self.sessions.get_mut(&params.session_id) else {
                        return JsonRpcResponse::error(
                            id,
                            ERROR_UNKNOWN_SESSION,
                            "unknown ACP session id",
                        );
                    };
                    let _cwd = &session.cwd;
                    let _path = &session.path;
                    session.prompt_count += 1;
                    let cancelled = session.cancelled;
                    session.cancelled = false;
                    if let Err(error) = persist_prompt_stub_messages(session, &prompt) {
                        return JsonRpcResponse::error(
                            id,
                            ERROR_SESSION_IO,
                            format!("failed to persist ACP prompt: {error}"),
                        );
                    }
                    (cancelled, session.prompt_count)
                };
                if !cancelled {
                    self.send_notification(
                        "session/update",
                        json!(SessionUpdateParams {
                            session_id: params.session_id.clone(),
                            update: self::protocol::SessionUpdate::AgentMessageChunk {
                                content: self::protocol::ContentBlock::Text {
                                    text: format!(
                                        "imp ACP scaffold received prompt: {}",
                                        prompt.chars().take(80).collect::<String>()
                                    ),
                                },
                            },
                        }),
                    )
                    .await;
                }
                JsonRpcResponse::success(
                    id,
                    json!(SessionPromptResult {
                        stop_reason: stop_reason_from_agent_status(None, cancelled),
                        meta: Some(json!({
                            "imp": {
                                "status": if cancelled { "cancelled" } else { "stubbed" },
                                "message": if cancelled { "prompt was cancelled before execution" } else { "agent turn wiring is not connected yet" },
                                "promptPreview": prompt.chars().take(120).collect::<String>(),
                                "promptCount": prompt_count,
                            }
                        })),
                    }),
                )
            }
            Err(error) => JsonRpcResponse::error(id, ERROR_UNSUPPORTED, error),
        }
    }
}

const ERROR_NOT_INITIALIZED: i64 = -32002;
const ERROR_UNKNOWN_SESSION: i64 = -32003;
const ERROR_UNSUPPORTED: i64 = -32004;
const ERROR_SESSION_IO: i64 = -32005;

fn create_session(cwd: &Path) -> imp_core::Result<(String, PathBuf, SessionManager)> {
    let session = SessionManager::new(cwd, &imp_core::storage::global_sessions_dir())?;
    let session_id = session.session_id().ok_or_else(|| {
        imp_core::error::Error::Session("new session did not have a persisted id".to_string())
    })?;
    let path = session
        .path()
        .ok_or_else(|| {
            imp_core::error::Error::Session("new session did not have a persisted path".to_string())
        })?
        .to_path_buf();
    Ok((session_id, path, session))
}

fn persist_prompt_stub_messages(session: &mut AcpSession, prompt: &str) -> imp_core::Result<()> {
    session.session.append(SessionEntry::Message {
        id: uuid::Uuid::new_v4().to_string(),
        parent_id: None,
        message: Message::user(prompt),
    })?;
    Ok(())
}

fn session_path_for_id(session_id: &str) -> Result<PathBuf, String> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return Err("invalid ACP session id".to_string());
    }
    Ok(imp_core::storage::global_sessions_dir().join(format!("{session_id}.jsonl")))
}

fn load_session_by_id(session_id: &str) -> Result<SessionManager, String> {
    let path = session_path_for_id(session_id)?;
    let session = SessionManager::open(&path).map_err(|error| error.to_string())?;
    match session.session_id().as_deref() {
        Some(opened_id) if opened_id == session_id => Ok(session),
        _ => Err("session id did not match opened session file".to_string()),
    }
}

fn method_name(request: &AgentRequest) -> &'static str {
    match request {
        AgentRequest::Initialize(_) => "initialize",
        AgentRequest::SessionNew(_) => "session/new",
        AgentRequest::SessionLoad(_) => "session/load",
        AgentRequest::SessionResume(_) => "session/resume",
        AgentRequest::SessionPrompt(_) => "session/prompt",
        AgentRequest::SessionClose(_) => "session/close",
    }
}

#[allow(dead_code)]
fn notification_name(notification: &AgentNotification) -> &'static str {
    match notification {
        AgentNotification::SessionCancel(_) => "session/cancel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[tokio::test]
    async fn stdio_server_initialize_handshake() {
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"test"}}}
"#;
        let mut output = Vec::new();

        run_stdio_server_with_io(&input[..], &mut output, "test-version")
            .await
            .unwrap();

        let line = String::from_utf8(output).unwrap();
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 1);
        assert_eq!(value["result"]["protocolVersion"], 1);
        assert_eq!(value["result"]["agentInfo"]["name"], "imp");
    }

    #[tokio::test]
    async fn stdio_server_rejects_session_before_initialize() {
        let input = br#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}
"#;
        let mut output = Vec::new();

        run_stdio_server_with_io(&input[..], &mut output, "test-version")
            .await
            .unwrap();

        let line = String::from_utf8(output).unwrap();
        let value: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["error"]["code"], ERROR_NOT_INITIALIZED);
    }

    #[tokio::test]
    async fn stdio_server_creates_session_after_initialize() {
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}
"#;
        let mut output = Vec::new();

        run_stdio_server_with_io(&input[..], &mut output, "test-version")
            .await
            .unwrap();

        let lines: Vec<_> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        let session_id = lines[1]["result"]["sessionId"].as_str().unwrap();
        assert!(!session_id.is_empty());
        assert!(session_path_for_id(session_id).unwrap().exists());
    }

    #[test]
    fn stdio_server_prompt_returns_stubbed_completion() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut server = AcpServer::new("test-version".to_string(), outbound_tx);

        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;
        rt.block_on(server.handle_line(init));
        assert!(rt.block_on(outbound_rx.recv()).is_some());

        let created = r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#;
        rt.block_on(server.handle_line(created));
        let created_value = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Response(response) => serde_json::to_value(response).unwrap(),
            other => panic!("expected response, got {other:?}"),
        };
        let session_id = created_value["result"]["sessionId"].as_str().unwrap();

        let prompt = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"hello"}}]}}}}"#,
            session_id
        );
        rt.block_on(server.handle_line(&prompt));
        let update = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Notification(notification) => {
                serde_json::to_value(notification).unwrap()
            }
            other => panic!("expected update notification, got {other:?}"),
        };
        let value = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Response(response) => serde_json::to_value(response).unwrap(),
            other => panic!("expected response, got {other:?}"),
        };

        assert_eq!(update["method"], "session/update");
        assert_eq!(value["result"]["stopReason"], "end_turn");
    }

    #[test]
    fn session_path_for_id_rejects_path_traversal() {
        assert!(session_path_for_id("../secret").is_err());
        assert!(session_path_for_id("nested/session").is_err());
    }

    #[test]
    fn create_session_uses_absolute_cwd_and_can_load_by_id() {
        let cwd = std::env::current_dir().unwrap();
        let (session_id, path, _session) = create_session(&cwd).unwrap();

        assert!(path.exists());
        let loaded = load_session_by_id(&session_id).unwrap();
        assert_eq!(loaded.session_id().as_deref(), Some(session_id.as_str()));
        let header = loaded.entries().first().unwrap();
        match header {
            imp_core::session::SessionEntry::Header {
                cwd: stored_cwd, ..
            } => {
                assert_eq!(stored_cwd, &cwd.to_string_lossy().to_string());
            }
            _ => panic!("expected session header"),
        }
    }

    #[test]
    fn session_prompt_persists_user_message_once() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut server = AcpServer::new("test-version".to_string(), outbound_tx);

        rt.block_on(server.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#));
        let _ = rt.block_on(outbound_rx.recv());
        rt.block_on(server.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#));
        let created_value = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Response(response) => serde_json::to_value(response).unwrap(),
            other => panic!("expected response, got {other:?}"),
        };
        let session_id = created_value["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_string();

        let prompt = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"persist me"}}]}}}}"#,
            session_id
        );
        rt.block_on(server.handle_line(&prompt));
        let _update = rt.block_on(outbound_rx.recv());
        let _response = rt.block_on(outbound_rx.recv());

        let loaded = load_session_by_id(&session_id).unwrap();
        let user_messages: Vec<_> = loaded
            .get_active_messages()
            .into_iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user),
                _ => None,
            })
            .collect();
        assert_eq!(user_messages.len(), 1);
        let text = match &user_messages[0].content[0] {
            imp_llm::ContentBlock::Text { text } => text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert_eq!(text, "persist me");
    }

    #[test]
    fn cancel_before_prompt_returns_cancelled_stop_reason() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut server = AcpServer::new("test-version".to_string(), outbound_tx);

        rt.block_on(server.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#));
        let _ = rt.block_on(outbound_rx.recv());
        rt.block_on(server.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#));
        let created_value = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Response(response) => serde_json::to_value(response).unwrap(),
            other => panic!("expected response, got {other:?}"),
        };
        let session_id = created_value["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_string();

        let cancel = format!(
            r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{}"}}}}"#,
            session_id
        );
        rt.block_on(server.handle_line(&cancel));

        let prompt = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"cancel me"}}]}}}}"#,
            session_id
        );
        rt.block_on(server.handle_line(&prompt));
        let value = match rt.block_on(outbound_rx.recv()).unwrap() {
            JsonRpcOutbound::Response(response) => serde_json::to_value(response).unwrap(),
            other => panic!("expected response, got {other:?}"),
        };

        assert_eq!(value["result"]["stopReason"], "cancelled");
    }
}
