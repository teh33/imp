use super::*;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use imp_llm::auth::{ApiKey, AuthStore};
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::Provider;
use tokio::sync::Mutex;

use crate::tools::{bash::BashTool, edit::EditTool, read::ReadTool, write::WriteTool};

// ── Shared test helpers (duplicated from unit tests to keep modules independent) ──

struct MockProvider {
    responses: Mutex<Vec<Vec<StreamEvent>>>,
}

impl MockProvider {
    fn new(responses: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        let mut responses = self.responses.try_lock().expect("MockProvider lock");
        let events = if responses.is_empty() {
            vec![StreamEvent::Error {
                error: "No more mock responses".to_string(),
            }]
        } else {
            responses.remove(0)
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok("mock-key".to_string())
    }

    fn id(&self) -> &str {
        "mock"
    }

    fn models(&self) -> &[ModelMeta] {
        &[]
    }
}

fn test_model(provider: Arc<dyn Provider>) -> Model {
    Model {
        meta: ModelMeta {
            id: "test-model".to_string(),
            provider: "mock".to_string(),
            name: "Test Model".to_string(),
            context_window: 200_000,
            max_output_tokens: 16_384,
            pricing: ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.3,
                cache_write_per_mtok: 3.75,
            },
            capabilities: Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        provider,
    }
}

fn text_response(text: &str, input_tokens: u32, output_tokens: u32) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            model: "test-model".to_string(),
        },
        StreamEvent::TextDelta {
            text: text.to_string(),
        },
        StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                usage: Some(Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::EndTurn,
                timestamp: 1000,
            },
        },
    ]
}

fn tool_call_response(
    call_id: &str,
    tool_name: &str,
    args: serde_json::Value,
    input_tokens: u32,
    output_tokens: u32,
) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            model: "test-model".to_string(),
        },
        StreamEvent::ToolCall {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: args.clone(),
        },
        StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: call_id.to_string(),
                    name: tool_name.to_string(),
                    arguments: args,
                }],
                usage: Some(Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::ToolUse,
                timestamp: 1000,
            },
        },
    ]
}

/// Create an agent pre-loaded with the reduced default tool set used by tests.
fn create_agent_with_tools(provider: Arc<dyn Provider>, cwd: PathBuf) -> (Agent, AgentHandle) {
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, cwd);
    agent.tools.register(Arc::new(WriteTool));
    agent.tools.register(Arc::new(ReadTool));
    agent.tools.register(Arc::new(EditTool));
    agent.tools.register(Arc::new(BashTool));
    (agent, handle)
}

/// Create an agent with reduced tools only (used for synthetic A/B tests).
fn create_agent_with_reduced_tools(
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
) -> (Agent, AgentHandle) {
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, cwd);
    agent.tools.register(Arc::new(WriteTool));
    agent.tools.register(Arc::new(ReadTool));
    agent.tools.register(Arc::new(EditTool));
    agent.tools.register(Arc::new(BashTool));
    (agent, handle)
}

// ── Test 1: Write then read a file ─────────────────────────────

#[tokio::test]
async fn agent_reads_and_writes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_write",
            "write",
            serde_json::json!({"path": "test.txt", "content": "hello world"}),
            100,
            20,
        ),
        tool_call_response(
            "call_read",
            "read",
            serde_json::json!({"path": "test.txt"}),
            100,
            20,
        ),
        text_response("The file contains: hello world", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent
        .run("Write and read a file".to_string())
        .await
        .unwrap();

    // File should exist on disk with correct content
    let on_disk = std::fs::read_to_string(tmp.path().join("test.txt")).unwrap();
    assert_eq!(on_disk, "hello world");

    // Read tool result should contain the file content
    let read_result = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_read" => Some(r),
            _ => None,
        })
        .expect("should have a read tool result");
    let read_text = read_result
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        read_text.contains("hello world"),
        "read result should contain file content, got: {read_text}"
    );

    // Assistant messages should include the write, read, and final text turns.
    let assistant_count = agent
        .messages
        .iter()
        .filter(|m| matches!(m, Message::Assistant(_)))
        .count();
    assert!(
        assistant_count >= 3,
        "got {assistant_count} assistant messages"
    );
}

// ── Test 2: Edit tool modifies a file ──────────────────────────

#[tokio::test]
async fn agent_edit_tool_modifies_file() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_write",
            "write",
            serde_json::json!({
                "path": "src/main.rs",
                "content": "fn main() {\n    println!(\"old\");\n}"
            }),
            100,
            20,
        ),
        tool_call_response(
            "call_edit",
            "edit",
            serde_json::json!({
                "path": "src/main.rs",
                "oldText": "old",
                "newText": "new"
            }),
            100,
            20,
        ),
        tool_call_response(
            "call_read",
            "read",
            serde_json::json!({"path": "src/main.rs"}),
            100,
            20,
        ),
        text_response("Done", 100, 20),
        text_response("Done", 100, 20),
        text_response("Done", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent.run("Edit a file".to_string()).await.unwrap();

    // File should contain "new" not "old"
    let on_disk = std::fs::read_to_string(tmp.path().join("src/main.rs")).unwrap();
    assert!(on_disk.contains("new"), "file should contain 'new'");
    assert!(!on_disk.contains("old"), "file should not contain 'old'");

    // Edit tool result should include a diff
    let edit_result = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_edit" => Some(r),
            _ => None,
        })
        .expect("should have an edit tool result");
    let edit_text = edit_result
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        edit_text.contains("---") || edit_text.contains("+++"),
        "edit result should include a diff, got: {edit_text}"
    );
}

// ── Test 3: Bash search finds a pattern (synthetic A/B baseline) ──────

#[tokio::test]
async fn agent_bash_search_finds_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("search_me.txt"),
        "line one\nunique_pattern_xyz here\nline three\n",
    )
    .unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_bash",
            "bash",
            serde_json::json!({"command": "grep --no-color -rn 'unique_pattern_xyz' ."}),
            100,
            20,
        ),
        text_response("Found it!", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
        text_response("Done.", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_reduced_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent.run("Search for a pattern".to_string()).await.unwrap();

    let bash_result = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_bash" => Some(r),
            _ => None,
        })
        .expect("should have a bash tool result");
    let bash_text = bash_result
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        !bash_text.trim().is_empty(),
        "bash grep output should not be empty"
    );
}

// ── Test 3b: repeated identical tool calls warn and then block ────────

#[tokio::test]
async fn agent_repeated_tool_calls_warn_then_block() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("repeat.txt"), "same content\n").unwrap();

    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "read",
            serde_json::json!({"path": "repeat.txt"}),
            100,
            20,
        ),
        tool_call_response(
            "call_2",
            "read",
            serde_json::json!({"path": "repeat.txt"}),
            100,
            20,
        ),
        tool_call_response(
            "call_3",
            "read",
            serde_json::json!({"path": "repeat.txt"}),
            100,
            20,
        ),
        tool_call_response(
            "call_4",
            "read",
            serde_json::json!({"path": "repeat.txt"}),
            100,
            20,
        ),
        text_response("Done", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_reduced_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent
        .run("Read the same file repeatedly".to_string())
        .await
        .unwrap();

    let third = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_3" => Some(r),
            _ => None,
        })
        .expect("third tool result");
    let fourth = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_4" => Some(r),
            _ => None,
        })
        .expect("fourth tool result");

    let third_text = third
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let fourth_text = fourth
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(third_text.contains("Warning: identical tool call repeated 3 times"));
    assert!(fourth.is_error);
    assert!(fourth_text.contains("Blocked: identical tool call repeated 4 times"));
    assert_eq!(
        agent
            .messages
            .iter()
            .filter(|message| matches!(message, Message::User(_)))
            .count(),
        1,
        "agent should stop after repeated-action block rather than enqueueing more follow-ups"
    );
}

#[test]
fn tool_results_indicate_repeated_action_detects_blocked_repeat_message() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_repeat".to_string(),
        tool_name: "read".to_string(),
        content: vec![ContentBlock::Text {
            text: "Blocked: identical tool call repeated 4 times in a row for 'read'.".to_string(),
        }],
        is_error: true,
        details: serde_json::Value::Null,
        timestamp: 0,
    };

    assert!(tool_results_indicate_repeated_action(&[result]));
}

// ── Test 4: Bash runs a command ────────────────────────────────

#[tokio::test]
async fn agent_bash_runs_command() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_bash",
            "bash",
            serde_json::json!({"command": "echo hello && echo world"}),
            100,
            20,
        ),
        text_response("Done", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent.run("Run a command".to_string()).await.unwrap();

    // Bash result should contain the command output
    let bash_result = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_bash" => Some(r),
            _ => None,
        })
        .expect("should have a bash tool result");
    let bash_text = bash_result
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        bash_text.contains("hello"),
        "bash output should contain 'hello', got: {bash_text}"
    );
    assert!(
        bash_text.contains("world"),
        "bash output should contain 'world', got: {bash_text}"
    );

    // Details should include exit_code: 0
    assert_eq!(bash_result.details["exit_code"], 0);
}

// ── Test 5: Tool error → agent self-corrects ───────────────────

#[tokio::test]
async fn agent_handles_tool_error_gracefully() {
    let tmp = tempfile::tempdir().unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_read",
            "read",
            serde_json::json!({"path": "nonexistent.txt"}),
            100,
            20,
        ),
        text_response("File not found, let me try something else", 100, 20),
    ]));

    let (mut agent, handle) = create_agent_with_tools(provider, tmp.path().to_path_buf());
    drop(handle);

    agent.run("Read a file".to_string()).await.unwrap();

    // Read tool result should have is_error=true
    let read_result = agent
        .messages
        .iter()
        .find_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "call_read" => Some(r),
            _ => None,
        })
        .expect("should have a read tool result");
    assert!(
        read_result.is_error,
        "reading nonexistent file should produce an error result"
    );

    // Agent should continue to turn 1 and self-correct with text
    let assistant_count = agent
        .messages
        .iter()
        .filter(|m| matches!(m, Message::Assistant(_)))
        .count();
    assert_eq!(
        assistant_count, 2,
        "agent should have 2 turns: error + recovery"
    );

    // Agent completed successfully (no Err return)
}
