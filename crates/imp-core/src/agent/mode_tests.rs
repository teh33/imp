use super::*;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use imp_llm::auth::{ApiKey, AuthStore};
use imp_llm::model::ModelMeta;
use imp_llm::provider::Provider;
use tokio::sync::Mutex;

// ── Mock provider (same shape as in tests) ─────────────────────

struct MockProvider {
    responses: Mutex<Vec<Vec<imp_llm::StreamEvent>>>,
}

impl MockProvider {
    fn new(responses: Vec<Vec<imp_llm::StreamEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn stream(
        &self,
        _model: &imp_llm::Model,
        _context: imp_llm::Context,
        _options: imp_llm::RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<imp_llm::StreamEvent>> + Send>> {
        let mut responses = self.responses.try_lock().expect("MockProvider lock");
        let events = if responses.is_empty() {
            vec![imp_llm::StreamEvent::Error {
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

    fn models(&self) -> &[imp_llm::model::ModelMeta] {
        &[]
    }
}

fn test_model(provider: Arc<dyn Provider>) -> imp_llm::Model {
    imp_llm::Model {
        meta: ModelMeta {
            id: "test-model".to_string(),
            provider: "mock".to_string(),
            name: "Test Model".to_string(),
            context_window: 200_000,
            max_output_tokens: 16_384,
            pricing: imp_llm::model::ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
                cache_read_per_mtok: 0.3,
                cache_write_per_mtok: 3.75,
            },
            capabilities: imp_llm::model::Capabilities {
                reasoning: true,
                images: false,
                tool_use: true,
            },
        },
        provider,
    }
}

fn text_response(text: &str, input: u32, output: u32) -> Vec<imp_llm::StreamEvent> {
    vec![
        imp_llm::StreamEvent::MessageStart {
            model: "test-model".to_string(),
        },
        imp_llm::StreamEvent::TextDelta {
            text: text.to_string(),
        },
        imp_llm::StreamEvent::MessageEnd {
            message: imp_llm::AssistantMessage {
                content: vec![imp_llm::ContentBlock::Text {
                    text: text.to_string(),
                }],
                usage: Some(imp_llm::Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: imp_llm::StopReason::EndTurn,
                timestamp: 1000,
            },
        },
    ]
}

fn tool_call_response(
    call_id: &str,
    tool_name: &str,
    args: serde_json::Value,
    input: u32,
    output: u32,
) -> Vec<imp_llm::StreamEvent> {
    vec![
        imp_llm::StreamEvent::MessageStart {
            model: "test-model".to_string(),
        },
        imp_llm::StreamEvent::ToolCall {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: args.clone(),
        },
        imp_llm::StreamEvent::MessageEnd {
            message: imp_llm::AssistantMessage {
                content: vec![imp_llm::ContentBlock::ToolCall {
                    id: call_id.to_string(),
                    name: tool_name.to_string(),
                    arguments: args,
                }],
                usage: Some(imp_llm::Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: imp_llm::StopReason::ToolUse,
                timestamp: 1000,
            },
        },
    ]
}

async fn collect_events(mut handle: AgentHandle) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = handle.event_rx.recv().await {
        events.push(event);
    }
    events
}

// ── Tool fixtures ───────────────────────────────────────────────

struct EchoTool;

#[async_trait]
impl crate::tools::Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn label(&self) -> &str {
        "Echo"
    }
    fn description(&self) -> &str {
        "Echoes back the input"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }
    fn is_readonly(&self) -> bool {
        true
    }
    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        let text = params["text"].as_str().unwrap_or("no text");
        Ok(crate::tools::ToolOutput::text(format!("echo: {text}")))
    }
}

struct NamedWriteTool(&'static str);

#[async_trait]
impl crate::tools::Tool for NamedWriteTool {
    fn name(&self) -> &str {
        self.0
    }
    fn label(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "A write tool"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"data": {"type": "string"}}})
    }
    fn is_readonly(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        _call_id: &str,
        _params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        Ok(crate::tools::ToolOutput::text("written"))
    }
}

fn single_text_model(text: &str) -> Arc<MockProvider> {
    Arc::new(MockProvider::new(vec![text_response(text, 50, 10)]))
}

/// Test: Full mode registers all tools (no filtering).
#[tokio::test]
async fn agent_mode_enforcement_full_registers_all_tools() {
    use crate::config::AgentMode;

    let provider = single_text_model("ok");
    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;

    // Register a mix of tools
    agent.tools.register(Arc::new(EchoTool)); // "echo" - not in any allow-list
    agent.tools.register(Arc::new(NamedWriteTool("write")));

    // Full mode allows everything — both tools should be present
    assert!(
        agent.tools.get("echo").is_some(),
        "echo should be registered"
    );
    assert!(
        agent.tools.get("write").is_some(),
        "write should be registered"
    );
    assert!(agent.mode.allows_tool("echo"));
    assert!(agent.mode.allows_tool("write"));
    assert!(agent.mode.allows_tool("any_future_tool"));
}

/// Test: Orchestrator mode excludes write-category tools at registration time.
#[test]
fn agent_mode_enforcement_orchestrator_excludes_write_tools() {
    use crate::config::AgentMode;
    use crate::tools::ToolRegistry;

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool)); // "echo" — not in orchestrator allow-list
    registry.register(Arc::new(NamedWriteTool("write")));
    registry.register(Arc::new(NamedWriteTool("edit")));
    registry.register(Arc::new(NamedWriteTool("bash")));

    // Apply the mode filter exactly as AgentBuilder would
    let mode = AgentMode::Orchestrator;
    registry.retain(|name| mode.allows_tool(name));

    // Write-category tools must be absent
    assert!(
        registry.get("write").is_none(),
        "write must be filtered out"
    );
    assert!(registry.get("edit").is_none(), "edit must be filtered out");
    assert!(registry.get("bash").is_none(), "bash must be filtered out");
    // echo is not in any mode allow-list either
    assert!(registry.get("echo").is_none(), "echo must be filtered out");
}

/// Test: Execution-time guard blocks a disallowed tool call and returns an error result.
#[tokio::test]
async fn agent_mode_enforcement_guard_blocks_disallowed() {
    use crate::config::AgentMode;

    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: model calls "write" — disallowed in orchestrator mode
        tool_call_response(
            "call_1",
            "write",
            serde_json::json!({"data": "content"}),
            50,
            10,
        ),
        // Turn 1: model responds after seeing the error
        text_response("Understood, I cannot write directly.", 50, 10),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Orchestrator;
    // Register write so it passes schema validation — the mode guard fires first
    agent.tools.register(Arc::new(NamedWriteTool("write")));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Write something".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // The tool execution end event should carry an error result
    let tool_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
    assert!(tool_end.is_some(), "should have a ToolExecutionEnd event");

    if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
        assert!(result.is_error, "mode guard should produce an error result");
        let text = result.content.iter().find_map(|c| {
            if let ContentBlock::Text { text } = c {
                Some(text.as_str())
            } else {
                None
            }
        });
        let text = text.expect("error result should have text");
        assert!(
            text.contains("write") && text.contains("mode"),
            "error should name the tool and mention mode, got: {text}"
        );
    }
}

/// Test: Execution-time guard allows a permitted tool call through cleanly.
#[tokio::test]
async fn agent_mode_enforcement_guard_allows_permitted() {
    use crate::config::AgentMode;

    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: model calls "read" — allowed in orchestrator mode
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            50,
            10,
        ),
        text_response("Echo succeeded", 50, 10),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    // Full mode keeps custom tools available
    agent.mode = AgentMode::Full;
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo something".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // Tool should have succeeded (not an error)
    let tool_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
    assert!(tool_end.is_some());

    if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
        assert!(!result.is_error, "permitted tool should succeed");
    }
}

/// Test: System prompt filters tool descriptions by mode.
#[test]
fn agent_mode_enforcement_system_prompt_filters() {
    use crate::config::AgentMode;
    use crate::system_prompt::{assemble, AssembleParams};
    use crate::tools::ToolRegistry;

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NamedWriteTool("write")));
    registry.register(Arc::new(NamedWriteTool("edit")));
    registry.register(Arc::new(NamedWriteTool("bash")));

    // Provide read-category tools too
    struct ReadTool;
    #[async_trait]
    impl crate::tools::Tool for ReadTool {
        fn name(&self) -> &str {
            "read"
        }
        fn label(&self) -> &str {
            "Read"
        }
        fn description(&self) -> &str {
            "Read a file"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_readonly(&self) -> bool {
            true
        }
        async fn execute(
            &self,
            _: &str,
            _: serde_json::Value,
            _: crate::tools::ToolContext,
        ) -> crate::error::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::text(""))
        }
    }
    registry.register(Arc::new(ReadTool));

    let mode = AgentMode::Orchestrator;
    let result = assemble(&AssembleParams {
        tools: &registry,
        agents_md: &[],
        skills: &[],
        facts: &[],
        project_memory_status: None,
        soul: None,
        task: None,
        role: None,
        mode: &mode,
        memory: None,
        user_profile: None,
        cwd: None,
        repo_context: None,
        learning_enabled: false,
        guardrail_profile: None,
    });

    // Orchestrator allows "read" — should appear in system prompt
    assert!(
        result.text.contains("- read:"),
        "read should be in orchestrator prompt"
    );

    // Write tools must be absent from the system prompt
    assert!(
        !result.text.contains("- write:"),
        "write must not appear in orchestrator prompt"
    );
    assert!(
        !result.text.contains("- edit:"),
        "edit must not appear in orchestrator prompt"
    );
    assert!(
        !result.text.contains("- bash:"),
        "bash must not appear in orchestrator prompt"
    );
}

/// Test: System prompt includes mode instructions for non-Full modes.
#[test]
fn agent_mode_enforcement_system_prompt_instructions() {
    use crate::config::AgentMode;
    use crate::system_prompt::{assemble, AssembleParams};
    use crate::tools::ToolRegistry;

    let registry = ToolRegistry::new();

    // Full mode — no extra instructions
    let full_result = assemble(&AssembleParams {
        tools: &registry,
        agents_md: &[],
        skills: &[],
        facts: &[],
        project_memory_status: None,
        soul: None,
        task: None,
        role: None,
        mode: &AgentMode::Full,
        memory: None,
        user_profile: None,
        cwd: None,
        repo_context: None,
        learning_enabled: false,
        guardrail_profile: None,
    });
    // Full mode has no instructions
    assert!(
        !full_result.text.contains("orchestrator"),
        "Full mode should not mention orchestrator"
    );
    assert!(
        !full_result.text.contains("You are a worker agent."),
        "Full mode should not include worker mode instructions"
    );

    // Orchestrator mode — should include mode instructions
    let orch_result = assemble(&AssembleParams {
        tools: &registry,
        agents_md: &[],
        skills: &[],
        facts: &[],
        project_memory_status: None,
        soul: None,
        task: None,
        role: None,
        mode: &AgentMode::Orchestrator,
        memory: None,
        user_profile: None,
        cwd: None,
        repo_context: None,
        learning_enabled: false,
        guardrail_profile: None,
    });
    assert!(
        orch_result.text.contains("orchestrator"),
        "orchestrator prompt should contain mode instructions, got: {}",
        orch_result.text
    );

    // Worker mode — should include mode instructions
    let worker_result = assemble(&AssembleParams {
        tools: &registry,
        agents_md: &[],
        skills: &[],
        facts: &[],
        project_memory_status: None,
        soul: None,
        task: None,
        role: None,
        mode: &AgentMode::Worker,
        memory: None,
        user_profile: None,
        cwd: None,
        repo_context: None,
        learning_enabled: false,
        guardrail_profile: None,
    });
    assert!(
        worker_result.text.contains("worker"),
        "worker prompt should contain mode instructions"
    );

    // Reviewer mode — should include mode instructions
    let reviewer_result = assemble(&AssembleParams {
        tools: &registry,
        agents_md: &[],
        skills: &[],
        facts: &[],
        project_memory_status: None,
        soul: None,
        task: None,
        role: None,
        mode: &AgentMode::Reviewer,
        memory: None,
        user_profile: None,
        cwd: None,
        repo_context: None,
        learning_enabled: false,
        guardrail_profile: None,
    });
    assert!(
        reviewer_result.text.contains("reviewer") || reviewer_result.text.contains("read"),
        "reviewer prompt should contain mode instructions"
    );
}
