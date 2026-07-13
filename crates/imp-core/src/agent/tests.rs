use super::*;
use crate::agent::turn_assessment::NextAction;
use crate::builder::AgentBuilder;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use imp_llm::auth::{ApiKey, AuthStore};
use imp_llm::model::{Capabilities, ModelMeta, ModelPricing};
use imp_llm::provider::Provider;
use imp_llm::ToolResultMessage;
use tokio::sync::{Mutex, Notify};

#[derive(Default)]
struct AnsweringUi {
    answer: String,
}

#[async_trait]
impl crate::ui::UserInterface for AnsweringUi {
    fn has_ui(&self) -> bool {
        true
    }

    async fn notify(&self, _: &str, _: crate::ui::NotifyLevel) {}

    async fn confirm(&self, _: &str, _: &str) -> Option<bool> {
        None
    }

    async fn select_with_context(
        &self,
        _: &str,
        _: &str,
        _: &[crate::ui::SelectOption],
    ) -> Option<usize> {
        None
    }

    async fn multi_select_with_context(
        &self,
        _: &str,
        _: &str,
        _: &[crate::ui::SelectOption],
    ) -> Option<Vec<usize>> {
        None
    }

    async fn input_with_context(&self, _: &str, _: &str, _: &str) -> Option<String> {
        Some(self.answer.clone())
    }

    async fn set_status(&self, _: &str, _: Option<&str>) {}

    async fn set_widget(&self, _: &str, _: Option<crate::ui::WidgetContent>) {}

    async fn custom(&self, _: crate::ui::ComponentSpec) -> Option<serde_json::Value> {
        None
    }
}

/// Each call to `stream()` pops the next response from the queue.
struct MockProvider {
    responses: Mutex<Vec<Vec<imp_llm::Result<StreamEvent>>>>,
    contexts: StdMutex<Vec<Context>>,
}

impl MockProvider {
    fn new(responses: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|events| events.into_iter().map(Ok).collect())
                    .collect(),
            ),
            contexts: StdMutex::new(Vec::new()),
        }
    }

    fn new_results(responses: Vec<Vec<imp_llm::Result<StreamEvent>>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            contexts: StdMutex::new(Vec::new()),
        }
    }

    fn contexts(&self) -> Vec<Context> {
        self.contexts
            .lock()
            .expect("MockProvider contexts lock")
            .clone()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn stream(
        &self,
        _model: &Model,
        context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        self.contexts
            .lock()
            .expect("MockProvider contexts lock")
            .push(context);
        // We need to get the next response synchronously. Use try_lock since
        // tests are single-threaded per agent run.
        let mut responses = self.responses.try_lock().expect("MockProvider lock");
        let events = if responses.is_empty() {
            vec![Ok(StreamEvent::Error {
                error: "No more mock responses".to_string(),
            })]
        } else {
            responses.remove(0)
        };
        let stream = futures::stream::iter(events);
        Box::pin(stream)
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

#[test]
fn workflow_controller_only_overrides_soft_completion() {
    use crate::agent::workflow_integration::workflow_layer_may_override_finish;

    assert!(workflow_layer_may_override_finish(&LoopDecision::Finish {
        status: RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        }
    }));
    assert!(workflow_layer_may_override_finish(&LoopDecision::Finish {
        status: RunFinalStatus::DoneWithConcerns {
            reason: StopReason::NoProgress,
            concerns: vec!["minor".into()],
        }
    }));
    assert!(!workflow_layer_may_override_finish(&LoopDecision::Finish {
        status: RunFinalStatus::Blocked {
            reason: StopReason::RepeatedAction,
            message: "repeated action".into(),
        }
    }));
    assert!(!workflow_layer_may_override_finish(&LoopDecision::Finish {
        status: RunFinalStatus::NeedsUserInput {
            question: "which task?".into(),
        }
    }));
    assert!(!workflow_layer_may_override_finish(
        &LoopDecision::Continue {
            reason: ContinueReason::WorkflowCloseout,
            prompt: "inspect graph".into(),
        }
    ));
}

#[test]
fn workflow_closeout_does_not_override_repeated_action_finish() {
    use crate::agent::loop_policy::{DefaultLoopPolicy, LoopPolicy};
    use crate::agent::turn_assessment::{
        PostTurnAssessment, RuntimeEvidence, TextFallbackEvidence, WorkflowEvidence,
    };
    use crate::agent::workflow_integration::workflow_layer_may_override_finish;

    let assessment = PostTurnAssessment {
        runtime: RuntimeEvidence {
            repeated_action: true,
            execution_stop_reason: None,
            work_completed: false,
            execution_debt: false,
            execution_evidence: false,
            planning_only_progress: false,
            orchestration_started: false,
        },
        workflow: WorkflowEvidence { stop_reason: None },
        text_fallback: TextFallbackEvidence {
            planner_stop_reason: None,
            execution_stop_reason: None,
        },
        continue_recommendation: None,
    };
    let decision = DefaultLoopPolicy.decide_after_turn(&assessment);

    assert!(matches!(
        decision,
        LoopDecision::Finish {
            status: RunFinalStatus::Blocked {
                reason: StopReason::RepeatedAction,
                ..
            }
        }
    ));
    assert!(!workflow_layer_may_override_finish(&decision));
}

#[tokio::test]
async fn workflow_closeout_can_follow_soft_done() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent
        .workflow_layer
        .controller_mut()
        .record_workflow_graph_changed();

    let decision = LoopDecision::Finish {
        status: RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        },
    };

    assert!(crate::agent::workflow_integration::workflow_layer_may_override_finish(&decision));
    assert!(matches!(
        agent.override_finish_with_workflow_decision(decision),
        LoopDecision::Continue {
            reason: ContinueReason::WorkflowCloseout,
            ..
        }
    ));
}

fn test_model(provider: Arc<dyn Provider>) -> Model {
    test_model_with_context_window(provider, 200_000)
}

fn test_model_with_context_window(provider: Arc<dyn Provider>, context_window: u32) -> Model {
    test_model_with_id_and_context_window(provider, "test-model", context_window)
}

fn test_model_with_id_and_context_window(
    provider: Arc<dyn Provider>,
    id: &str,
    context_window: u32,
) -> Model {
    Model {
        meta: ModelMeta {
            id: id.to_string(),
            provider: "mock".to_string(),
            name: "Test Model".to_string(),
            context_window,
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

fn large_window_test_model(provider: Arc<dyn Provider>) -> Model {
    test_model_with_id_and_context_window(provider, "large-window-test-model", 1_050_000)
}

fn tool_heavy_messages_for_usage(model: &Model, target_tokens: u32) -> Vec<Message> {
    let mut messages = Vec::new();
    let chunk = "abcdefghijklmnopqrstuvwxyz0123456789 ".repeat(4_000);
    let mut index = 0usize;
    while crate::context::context_usage(&messages, model).used < target_tokens {
        let call_id = format!("usage_call_{index}");
        messages.push(Message::user(format!("inspect generated file {index}")));
        messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/generated_{index}.rs")}),
        ));
        messages.push(make_tool_result(&call_id, "read", &chunk));
        index += 1;
    }
    messages
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

fn context_length_exceeded_response() -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            model: "test-model".to_string(),
        },
        StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: Vec::new(),
                usage: Some(Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::Error(
                    "context_length_exceeded: Your input exceeds the context window of this model. Please adjust your input and try again."
                        .to_string(),
                ),
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

fn multi_tool_call_response(
    calls: &[(&str, &str, serde_json::Value)],
    input_tokens: u32,
    output_tokens: u32,
) -> Vec<StreamEvent> {
    let mut events = vec![StreamEvent::MessageStart {
        model: "test-model".to_string(),
    }];

    let mut content = Vec::new();
    for (id, name, args) in calls {
        events.push(StreamEvent::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.clone(),
        });
        content.push(ContentBlock::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.clone(),
        });
    }

    events.push(StreamEvent::MessageEnd {
        message: AssistantMessage {
            content,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            }),
            stop_reason: LlmStopReason::ToolUse,
            timestamp: 1000,
        },
    });

    events
}

fn make_assistant_tool_call(call_id: &str, tool_name: &str, args: serde_json::Value) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            arguments: args,
        }],
        usage: None,
        stop_reason: LlmStopReason::ToolUse,
        timestamp: imp_llm::now(),
    })
}

fn make_tool_result(call_id: &str, tool_name: &str, output: &str) -> Message {
    Message::ToolResult(imp_llm::ToolResultMessage {
        tool_call_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        content: vec![ContentBlock::Text {
            text: output.to_string(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: imp_llm::now(),
    })
}

fn tool_result_text(message: &Message) -> Option<&str> {
    match message {
        Message::ToolResult(result) => result.content.iter().find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }),
        _ => None,
    }
}

/// A simple echo tool for testing.
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
            "properties": {
                "text": { "type": "string" }
            },
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

struct DurableWorkTool;

#[async_trait]
impl crate::tools::Tool for DurableWorkTool {
    fn name(&self) -> &str {
        "work"
    }

    fn label(&self) -> &str {
        "Work"
    }

    fn description(&self) -> &str {
        "Mock durable workflow tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "action": { "type": "string" } },
            "required": ["action"]
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        call_id: &str,
        params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        let action = params
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        Ok(crate::tools::ToolOutput {
            content: vec![ContentBlock::Text {
                text: format!("work {action} ok"),
            }],
            details: serde_json::json!({
                "action": action,
                "id": format!("task-{call_id}"),
                "item": { "id": format!("task-{call_id}"), "status": "open" }
            }),
            is_error: false,
        })
    }
}

struct ExtensionNetworkTool;

#[async_trait]
impl crate::tools::Tool for ExtensionNetworkTool {
    fn name(&self) -> &str {
        "extension_net"
    }
    fn label(&self) -> &str {
        "Extension network"
    }
    fn description(&self) -> &str {
        "Pretends to perform extension network access"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn is_readonly(&self) -> bool {
        false
    }
    fn policy_metadata(&self) -> crate::reference_monitor::ToolMetadata {
        let mut metadata = crate::reference_monitor::ToolMetadata::new(
            self.name(),
            crate::reference_monitor::ToolActionKind::Extension,
        );
        metadata.extension = true;
        metadata.extension_id = Some("test.extension".into());
        metadata.network = true;
        metadata.requires_approval = true;
        metadata
    }
    async fn execute(
        &self,
        _call_id: &str,
        _params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        Ok(crate::tools::ToolOutput::text("network extension executed"))
    }
}

/// A mutable tool for testing write partitioning.
#[allow(dead_code)]
struct WriteTool;

#[async_trait]
impl crate::tools::Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn label(&self) -> &str {
        "Write"
    }
    fn description(&self) -> &str {
        "Writes data"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": { "type": "string" }
            },
            "required": ["data"]
        })
    }
    fn is_readonly(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        let data = params["data"].as_str().unwrap_or("no data");
        Ok(crate::tools::ToolOutput::text(format!("wrote: {data}")))
    }
}

struct ConcurrentReadonlyState {
    readonly_expected: usize,
    readonly_started: AtomicUsize,
    readonly_finished: AtomicUsize,
    mutable_observed_finished: AtomicUsize,
    log: StdMutex<Vec<String>>,
    notify: Notify,
}

impl ConcurrentReadonlyState {
    fn new(readonly_expected: usize) -> Self {
        Self {
            readonly_expected,
            readonly_started: AtomicUsize::new(0),
            readonly_finished: AtomicUsize::new(0),
            mutable_observed_finished: AtomicUsize::new(0),
            log: StdMutex::new(Vec::new()),
            notify: Notify::new(),
        }
    }

    fn record(&self, entry: impl Into<String>) {
        self.log
            .lock()
            .expect("concurrent log lock")
            .push(entry.into());
    }

    async fn wait_for_all_readonly_to_start(&self) {
        while self.readonly_started.load(Ordering::SeqCst) < self.readonly_expected {
            self.notify.notified().await;
        }
    }
}

struct CoordinatedReadonlyTool {
    name: &'static str,
    shared: Arc<ConcurrentReadonlyState>,
}

#[async_trait]
impl crate::tools::Tool for CoordinatedReadonlyTool {
    fn name(&self) -> &str {
        self.name
    }
    fn label(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Read-only tool used to verify concurrent execution"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
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
        self.shared.record(format!("{}:start", self.name));
        self.shared.readonly_started.fetch_add(1, Ordering::SeqCst);
        self.shared.notify.notify_waiters();
        self.shared.wait_for_all_readonly_to_start().await;
        self.shared.record(format!("{}:end", self.name));
        self.shared.readonly_finished.fetch_add(1, Ordering::SeqCst);

        let text = params["text"].as_str().unwrap_or(self.name);
        Ok(crate::tools::ToolOutput::text(format!(
            "{}: {text}",
            self.name
        )))
    }
}

struct CoordinatedMutableTool {
    shared: Arc<ConcurrentReadonlyState>,
}

#[async_trait]
impl crate::tools::Tool for CoordinatedMutableTool {
    fn name(&self) -> &str {
        "write_after_reads"
    }
    fn label(&self) -> &str {
        "Write After Reads"
    }
    fn description(&self) -> &str {
        "Mutable tool used to verify read-only tools finish first"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": { "type": "string" }
            },
            "required": ["data"]
        })
    }
    fn is_readonly(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        _ctx: crate::tools::ToolContext,
    ) -> crate::error::Result<crate::tools::ToolOutput> {
        let finished = self.shared.readonly_finished.load(Ordering::SeqCst);
        self.shared
            .mutable_observed_finished
            .store(finished, Ordering::SeqCst);
        self.shared.record("write_after_reads:start");

        let data = params["data"].as_str().unwrap_or("no data");
        Ok(crate::tools::ToolOutput::text(format!(
            "wrote after reads: {data}"
        )))
    }
}

/// Collect all events from the handle until the channel closes.
async fn collect_events(mut handle: AgentHandle) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = handle.event_rx.recv().await {
        events.push(event);
    }
    events
}

#[test]
fn agent_queues_workflow_hint_for_planner_requests() {
    let provider = Arc::new(MockProvider::new(vec![
        text_response("Loaded workflow skill", 100, 20),
        text_response("Done", 120, 25),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_skill_available(true);
    agent.mode = AgentMode::Planner;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        agent
            .run("Please split this into units for workers".to_string())
            .await
            .unwrap();
    });

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts.len(), 1);
    assert_eq!(user_texts[0], "Please split this into units for workers");
}

#[tokio::test]
async fn agent_queues_workflow_externalization_follow_up_after_planning_turn() {
    let provider = Arc::new(MockProvider::new(vec![
            text_response(
                "Here is the rollout decomposition: split this into phases and tasks, add dependencies, and define verification steps.",
                100,
                20,
            ),
            text_response("Externalized into workflow.", 120, 25),
        ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_skill_available(true);
    agent.mode = AgentMode::Planner;

    agent
        .run("Please split this rollout into tasks".to_string())
        .await
        .unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts.len(), 2);
    assert_eq!(user_texts[0], "Please split this rollout into tasks");
    assert!(user_texts[1].contains("explicitly asked for durable work structure"));
}

#[tokio::test]
async fn agent_does_not_externalize_explanatory_taxonomy_answers() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
            "Skills are reusable playbooks. Workflows are process lanes. Subagents are delegated workers. They compose, but they are not interchangeable.",
            100,
            20,
        )]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_skill_available(true);
    agent.mode = AgentMode::Planner;

    agent
        .run("How do workflows differ from skills or subagents?".to_string())
        .await
        .unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        user_texts,
        vec!["How do workflows differ from skills or subagents?".to_string()]
    );
}

#[test]
fn externalization_follow_up_requires_user_externalization_intent() {
    let explanatory = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Skills are reusable playbooks. Workflows are process lanes. Subagents are delegated workers.".into(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 0,
        };
    let mut agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    agent.mode = AgentMode::Planner;
    agent.set_workflow_skill_available(true);
    assert!(!agent.should_queue_workflow_externalization_for_test(
        &explanatory,
        "How do workflows differ from skills or subagents?",
    ));

    let plan = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Here is the rollout decomposition: split this into phases and tasks, add dependencies, and define verification steps.".into(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 0,
        };
    assert!(agent.should_queue_workflow_externalization_for_test(
        &plan,
        "Please split this rollout into tasks",
    ));
}

#[tokio::test]
async fn turn_assessment_debug_view_reports_execution_blocker() {
    let (agent, _handle) = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    );
    let assessment = agent.assess_post_turn(
        &AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "Verify failed.".to_string(),
            }],
            usage: None,
            stop_reason: LlmStopReason::EndTurn,
            timestamp: 0,
        },
        &[imp_llm::ToolResultMessage {
            tool_call_id: "call_verify".to_string(),
            tool_name: "workflow".to_string(),
            content: vec![ContentBlock::Text {
                text: "Verify failed".to_string(),
            }],
            is_error: true,
            details: serde_json::json!({
                "action": "verify",
                "passed": false,
                "exit_code": 1
            }),
            timestamp: 0,
        }],
        true,
        &TurnWorkflowReview::no_change(0),
    );

    let debug = assessment.debug_view();
    assert_eq!(
        debug.runtime.execution_stop_reason.as_deref(),
        Some("execution_blocked")
    );
    assert_eq!(
        debug.chosen_action,
        NextActionDebugView::Stop {
            reason: "execution_blocked".to_string(),
        }
    );
}

#[test]
fn turn_assessment_debug_view_reports_continue_recommendation() {
    let assessment = PostTurnAssessment {
        runtime: RuntimeEvidence {
            repeated_action: false,
            execution_stop_reason: None,
            work_completed: false,
            execution_debt: false,
            execution_evidence: false,
            planning_only_progress: false,
            orchestration_started: false,
        },
        workflow: WorkflowEvidence { stop_reason: None },
        text_fallback: TextFallbackEvidence {
            planner_stop_reason: None,
            execution_stop_reason: None,
        },
        continue_recommendation: Some(ContinueRecommendation {
            prompt: "continue".to_string(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        }),
    };

    let debug = assessment.debug_view();
    let recommendation = debug
        .continue_recommendation
        .expect("continue recommendation present");
    assert_eq!(recommendation.reason, "high_confidence_visible_next_step");
    assert!(matches!(
        debug.chosen_action,
        NextActionDebugView::Continue { .. }
    ));
}

#[tokio::test]
async fn agent_run_artifacts_writes_trace_and_evidence_packet() {
    let temp = tempfile::TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 10, 5)]));
    let model = test_model(provider);
    let (mut agent, _handle) = AgentBuilder::new(
        Config::default(),
        temp.path().to_path_buf(),
        model,
        String::new(),
    )
    .verify_command("printf verify-ok", true)
    .build()
    .unwrap();
    agent.workflow_contract_mut().autonomy_mode = crate::workflow::AutonomyMode::AllowAll;

    agent.run("Do the work".to_string()).await.unwrap();

    let runs_dir = temp.path().join(".imp").join("runs");
    let mut runs = std::fs::read_dir(&runs_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run_dir = runs.pop().unwrap().path();
    let trace = std::fs::read_to_string(run_dir.join("trace.jsonl")).unwrap();
    assert!(trace.contains("agent.start"));
    assert!(trace.contains("agent.end"));
    let evidence = std::fs::read_to_string(run_dir.join("evidence.md")).unwrap();
    assert!(evidence.contains("# Evidence Packet"));
    assert!(evidence.contains("Do the work"));
    assert!(evidence.contains("config policy:"));
    assert!(evidence.contains("hard-rail bypass: none recorded"));
    assert!(evidence.contains("policy.checked trace events"));
    assert!(evidence.contains("trace.jsonl"));
    assert!(evidence.contains("verify-ok"));
    assert!(evidence.contains("passed"));
    assert!(run_dir.join("verification/verify-1/status.json").exists());
    assert!(run_dir.join("workflow-contract.json").exists());
}

#[tokio::test]
async fn default_safe_compatibility_allows_readonly_tool() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_read",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy = first_policy_record(&events).expect("policy checked");
    assert!(policy.decision.is_allowed());
    let result = first_tool_result(&events).expect("tool end event");
    assert!(!result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("echo: hello")
    );
}

#[tokio::test]
async fn default_safe_compatibility_allows_write_tool() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_write",
            "write",
            serde_json::json!({"data": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
        text_response("done", 100, 10),
        text_response("done", 100, 10),
        text_response("done", 100, 10),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(WriteTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Write hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy = first_policy_record(&events).expect("policy checked");
    assert!(policy.decision.is_allowed());
    let result = first_tool_result(&events).expect("tool end event");
    assert!(!result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("wrote: hello")
    );
}

#[tokio::test]
async fn default_safe_compatibility_preserves_run_policy_tool_deny() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_echo",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));
    agent.run_policy = crate::policy::RunPolicy::new().deny_tool("echo");

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy = first_policy_record(&events).expect("policy checked");
    assert!(matches!(
        policy.decision,
        crate::reference_monitor::ToolPolicyDecision::Deny { .. }
    ));
    let result = first_tool_result(&events).expect("tool end event");
    assert!(result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("Tool `echo` denied by run policy.")
    );
}

#[tokio::test]
async fn default_safe_compatibility_preserves_agent_mode_tool_deny() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_write",
            "write",
            serde_json::json!({"data": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Reviewer;
    agent.tools.register(Arc::new(WriteTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Write hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy = first_policy_record(&events).expect("policy checked");
    assert!(matches!(
        policy.decision,
        crate::reference_monitor::ToolPolicyDecision::Deny { .. }
    ));
    let result = first_tool_result(&events).expect("tool end event");
    assert!(result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("Tool 'write' is not available in reviewer mode")
    );
}

fn first_policy_record(
    events: &[AgentEvent],
) -> Option<&crate::reference_monitor::PolicyTraceRecord> {
    events.iter().find_map(|event| match event {
        AgentEvent::PolicyChecked { record } => Some(record),
        _ => None,
    })
}

fn first_tool_result(events: &[AgentEvent]) -> Option<&ToolResultMessage> {
    events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
        _ => None,
    })
}

#[tokio::test]
async fn extension_network_policy_denial_attaches_policy_details_to_result() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response("call_ext", "extension_net", serde_json::json!({}), 100, 30),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;
    agent.tools.register(Arc::new(ExtensionNetworkTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Run extension".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy = first_policy_record(&events).expect("policy checked");
    assert_eq!(policy.tool_name, "extension_net");
    assert!(matches!(
        policy.decision,
        crate::reference_monitor::ToolPolicyDecision::Deny { .. }
    ));
    let result = first_tool_result(&events).expect("tool result");
    assert!(result.is_error);
    assert_eq!(result.details["policy"]["tool_name"], "extension_net");
    assert_eq!(
        result.details["policy"]["decision"]["reason"]["code"],
        "policy_extension_network_denied"
    );
}

#[tokio::test]
async fn tool_execution_policy_routes_run_policy_deny_through_reference_monitor() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));
    agent.run_policy = crate::policy::RunPolicy::new().deny_tool("echo");

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy_event = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::PolicyChecked { record } => Some(record),
            _ => None,
        })
        .expect("policy checked event");
    assert_eq!(policy_event.tool_name, "echo");
    assert!(policy_event.args_hash.is_some());
    assert!(matches!(
        policy_event.decision,
        crate::reference_monitor::ToolPolicyDecision::Deny { .. }
    ));

    let result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
            _ => None,
        })
        .expect("tool end event");
    assert!(result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("Tool `echo` denied by run policy.")
    );

    let checkpoint = events.iter().rev().find_map(|event| match event {
        AgentEvent::RecoveryCheckpoint { checkpoint } => checkpoint.error_class.as_deref(),
        _ => None,
    });
    assert_eq!(checkpoint, Some("run_policy_blocked"));
}

#[tokio::test]
async fn tool_execution_policy_routes_agent_mode_deny_through_reference_monitor() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "write",
            serde_json::json!({"data": "hello"}),
            100,
            30,
        ),
        text_response("done", 100, 10),
    ]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Reviewer;
    agent.tools.register(Arc::new(WriteTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Write hello".to_string()).await.unwrap();
    drop(agent);
    let events = events_task.await.unwrap();

    let policy_event = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::PolicyChecked { record } => Some(record),
            _ => None,
        })
        .expect("policy checked event");
    assert_eq!(policy_event.tool_name, "write");
    assert!(matches!(
        policy_event.decision,
        crate::reference_monitor::ToolPolicyDecision::Deny { .. }
    ));

    let result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
            _ => None,
        })
        .expect("tool end event");
    assert!(result.is_error);
    assert_eq!(
        tool_result_text(&Message::ToolResult(result.clone())),
        Some("Tool 'write' is not available in reviewer mode")
    );

    let checkpoint = events.iter().rev().find_map(|event| match event {
        AgentEvent::RecoveryCheckpoint { checkpoint } => checkpoint.error_class.as_deref(),
        _ => None,
    });
    assert_eq!(checkpoint, Some("mode_blocked"));
}

#[tokio::test]
async fn emits_turn_assessment_event_for_execution_blocker() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_check",
            "bash",
            serde_json::json!({"command": "cargo check -p definitely_missing_crate", "timeout": 1}),
            100,
            20,
        ),
        text_response("The check failed.", 120, 20),
        text_response("Recovery follow-up noted.", 120, 20),
        text_response("Recovery complete.", 120, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;
    agent.tools.register(Arc::new(crate::tools::bash::BashTool));

    let events_task = tokio::spawn(collect_events(handle));
    let _ = agent.run("Run the check".to_string()).await;
    drop(agent);
    let events = events_task.await.unwrap();

    let assessment = events.iter().find_map(|event| match event {
        AgentEvent::TurnAssessment { assessment, .. } => Some(assessment),
        _ => None,
    });

    let assessment = assessment.expect("turn assessment emitted");
    assert_eq!(assessment.runtime.execution_stop_reason.as_deref(), None);
    let recommendation = assessment
        .continue_recommendation
        .as_ref()
        .expect("failed bash check should request recovery follow-up");
    assert_eq!(recommendation.reason, "execution_debt");
    assert_eq!(
        assessment.chosen_action,
        NextActionDebugView::Continue {
            prompt: failed_bash_recovery_follow_up_text().to_string(),
            reason: "execution_debt".to_string(),
        }
    );
}

#[tokio::test]
async fn failed_bash_command_with_completion_text_continues_for_recovery() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_bash",
            "bash",
            serde_json::json!({"command": "false", "timeout": 1}),
            100,
            20,
        ),
        text_response("Done.", 120, 20),
        text_response("Recovery follow-up noted.", 120, 20),
        text_response("Recovery complete.", 120, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;
    agent.tools.register(Arc::new(crate::tools::bash::BashTool));

    let _ = agent
        .run("Run the command and recover if it fails".to_string())
        .await;

    let user_follow_up = agent.messages.iter().any(|message| {
        matches!(
            message,
            Message::User(user) if user.content.iter().any(|block| matches!(
                block,
                ContentBlock::Text { text } if text == failed_bash_recovery_follow_up_text()
            ))
        )
    });
    assert!(
            user_follow_up,
            "failed bash output should queue a recovery follow-up before completion text can end the run"
        );
}

#[test]
fn post_turn_assessment_prefers_execution_blocker_over_completion() {
    let assessment = PostTurnAssessment {
        runtime: RuntimeEvidence {
            repeated_action: false,
            execution_stop_reason: Some(StopReason::ExecutionBlocked),
            work_completed: true,
            execution_debt: false,
            execution_evidence: false,
            planning_only_progress: false,
            orchestration_started: false,
        },
        workflow: WorkflowEvidence {
            stop_reason: Some(StopReason::DecompositionCompleted),
        },
        text_fallback: TextFallbackEvidence {
            planner_stop_reason: Some(StopReason::DecompositionCompleted),
            execution_stop_reason: Some(StopReason::WorkCompleted),
        },
        continue_recommendation: Some(ContinueRecommendation {
            prompt: "continue".to_string(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        }),
    };

    assert_eq!(
        assessment.into_next_action(),
        NextAction::Stop {
            reason: StopReason::ExecutionBlocked,
        }
    );
}

#[test]
fn post_turn_assessment_emits_continue_when_no_stop_reason_exists() {
    let assessment = PostTurnAssessment {
        runtime: RuntimeEvidence {
            repeated_action: false,
            execution_stop_reason: None,
            work_completed: false,
            execution_debt: false,
            execution_evidence: false,
            planning_only_progress: false,
            orchestration_started: false,
        },
        workflow: WorkflowEvidence { stop_reason: None },
        text_fallback: TextFallbackEvidence {
            planner_stop_reason: None,
            execution_stop_reason: None,
        },
        continue_recommendation: Some(ContinueRecommendation {
            prompt: "continue".to_string(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        }),
    };

    assert_eq!(
        assessment.into_next_action(),
        NextAction::Continue {
            prompt: "continue".to_string(),
            reason: ContinueReason::HighConfidenceVisibleNextStep,
        }
    );
}

#[test]
fn execution_debt_follow_up_is_preferred_before_stopping_for_planning_only_progress() {
    let assessment = PostTurnAssessment {
        runtime: RuntimeEvidence {
            repeated_action: false,
            execution_stop_reason: None,
            work_completed: false,
            execution_debt: true,
            execution_evidence: false,
            planning_only_progress: false,
            orchestration_started: false,
        },
        workflow: WorkflowEvidence { stop_reason: None },
        text_fallback: TextFallbackEvidence {
            planner_stop_reason: None,
            execution_stop_reason: None,
        },
        continue_recommendation: Some(ContinueRecommendation {
            prompt: execution_debt_follow_up_text().to_string(),
            reason: ContinueReason::ExecutionDebt,
        }),
    };

    assert_eq!(
        assessment.into_next_action(),
        NextAction::Continue {
            prompt: execution_debt_follow_up_text().to_string(),
            reason: ContinueReason::ExecutionDebt,
        }
    );
}

#[test]
fn workflow_planning_without_execution_creates_execution_debt_follow_up() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Created task".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "action": "create" }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert!(agent.workflow_execution_debt_for_test(std::slice::from_ref(&result)));
    assert!(!agent.workflow_execution_evidence_for_test(std::slice::from_ref(&result)));
    assert!(should_queue_execution_debt_follow_up(
        true, false, false, true
    ));
}

#[test]
fn native_work_planning_without_execution_creates_execution_debt_follow_up() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_work".to_string(),
        tool_name: "work".to_string(),
        content: vec![ContentBlock::Text {
            text: "Created task".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "action": "create" }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert!(agent.workflow_execution_debt_for_test(std::slice::from_ref(&result)));
    assert!(!agent.workflow_execution_evidence_for_test(std::slice::from_ref(&result)));
    assert!(should_queue_execution_debt_follow_up(
        true, false, false, true
    ));
}

#[test]
fn agent_can_resume_workflow_controller_from_project_run_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = crate::storage::project_run_artifacts(temp.path(), "run_resume").unwrap();
    let mut controller = crate::workflow::WorkflowRunController::new();
    controller.record_workflow_graph_changed();
    controller
        .save_to_path(&artifacts.workflow_controller_path())
        .unwrap();

    let (mut agent, _handle) = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        temp.path().to_path_buf(),
    );
    agent
        .resume_workflow_controller_from_project_run("run_resume")
        .unwrap();

    assert!(agent.workflow_layer.controller().graph_closeout_required);
}

#[test]
fn workflow_run_status_result_extracts_terminal_status() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "run done".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "action": "run_state",
            "run_id": "run-42",
            "status": "done",
            "summary": { "total_failed": 0 }
        }),
        timestamp: 0,
    };

    assert_eq!(
        crate::agent::workflow_integration::workflow_run_status_from_result(&result),
        Some((
            "run-42".into(),
            crate::workflow::WorkflowChildRunStatus::Done
        ))
    );
}

#[test]
fn successful_check_command_is_direct_closeout_evidence() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_bash".to_string(),
        tool_name: "bash".to_string(),
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "command": "cargo test -p imp-core workflow::controller --lib",
            "exit_code": 0
        }),
        timestamp: 0,
    };

    assert!(bash_result_is_successful_check(&result));
}

#[test]
fn workflow_run_result_extracts_run_id_for_supervision() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Started native workflow orchestration".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "action": "run", "run_id": "run-42" }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert_eq!(
        agent
            .workflow_orchestration_run_id_for_test(std::slice::from_ref(&result))
            .as_deref(),
        Some("run-42")
    );
    assert!(agent.workflow_orchestration_started_for_test(std::slice::from_ref(&result)));
}

#[test]
fn native_workflow_run_result_without_child_run_id_counts_as_orchestration() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Workflow needs main agent action.".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "action": "run",
            "id": "agent-action-workflow",
            "status": "pending",
            "result": {
                "next_action": { "kind": "agent_action" }
            }
        }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert_eq!(
        agent.workflow_orchestration_run_id_for_test(std::slice::from_ref(&result)),
        None
    );
    assert!(agent.workflow_orchestration_started_for_test(std::slice::from_ref(&result)));
}

#[test]
fn workflow_run_assessment_prefers_supervision_over_work_completed() {
    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Started native workflow orchestration".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "action": "run", "run_id": "run-42" }),
        timestamp: 0,
    };
    let message = AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "Started run.".to_string(),
        }],
        usage: None,
        stop_reason: LlmStopReason::ToolUse,
        timestamp: 0,
    };

    let assessment = agent.assess_post_turn(
        &message,
        std::slice::from_ref(&result),
        true,
        &TurnWorkflowReview::no_change(0),
    );

    assert!(assessment.runtime.orchestration_started);
    assert_eq!(
        assessment.into_next_action(),
        NextAction::Continue {
            prompt: agent
                .workflow_continue_recommendation(&agent.workflow_post_turn_signals(
                    std::slice::from_ref(&result),
                    &TurnWorkflowReview::no_change(0),
                ))
                .expect("workflow recommendation")
                .prompt,
            reason: ContinueReason::OrchestrationProgress,
        }
    );
}

#[test]
fn mutating_tool_call_satisfies_execution_evidence() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_edit".to_string(),
        tool_name: "edit".to_string(),
        content: vec![ContentBlock::Text {
            text: "diff".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "path": "src/lib.rs" }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert!(agent.workflow_execution_evidence_for_test(&[result]));
    assert!(!should_queue_execution_debt_follow_up(
        true, true, false, true
    ));
}

#[test]
fn tool_results_indicate_execution_blocker_detects_failed_verify() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_verify".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Verify failed".to_string(),
        }],
        is_error: true,
        details: serde_json::json!({
            "action": "verify",
            "passed": false,
            "exit_code": 1
        }),
        timestamp: 0,
    };

    assert_eq!(
        tool_results_indicate_execution_blocker(&[result], AgentMode::Full),
        Some(StopReason::ExecutionBlocked)
    );
}

#[test]
fn failed_bash_check_is_recovery_signal_not_immediate_blocker() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_bash".to_string(),
        tool_name: "bash".to_string(),
        content: vec![ContentBlock::Text {
            text: "cargo test failed".to_string(),
        }],
        is_error: true,
        details: serde_json::json!({
            "command": "cargo test -p imp-core",
            "exit_code": 101
        }),
        timestamp: 0,
    };

    assert_eq!(
        tool_results_indicate_execution_blocker(std::slice::from_ref(&result), AgentMode::Full),
        None
    );
    assert!(tool_results_indicate_failed_bash_command(
        &[result],
        AgentMode::Full
    ));
}

#[test]
fn failed_bash_after_successful_edit_is_recovery_signal_not_runtime_blocker() {
    let edit_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_edit".to_string(),
        tool_name: "edit".to_string(),
        content: vec![ContentBlock::Text {
            text: "edited file".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "path": "src/lib.rs" }),
        timestamp: 0,
    };
    let bash_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_bash".to_string(),
        tool_name: "bash".to_string(),
        content: vec![ContentBlock::Text {
            text: "compiler error".to_string(),
        }],
        is_error: true,
        details: serde_json::json!({
            "command": "cargo check -p imp-core",
            "exit_code": 101
        }),
        timestamp: 0,
    };

    assert_eq!(
        tool_results_indicate_execution_blocker(
            &[edit_result, bash_result.clone()],
            AgentMode::Full
        ),
        None
    );
    assert!(tool_results_indicate_failed_bash_command(
        &[bash_result],
        AgentMode::Full
    ));
}

#[test]
fn records_and_resolves_edited_files_verification_obligation() {
    let provider = Arc::new(MockProvider::new(vec![]));
    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;

    let edit_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_edit".to_string(),
        tool_name: "edit".to_string(),
        content: vec![ContentBlock::Text {
            text: "edited file".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({ "path": "src/lib.rs" }),
        timestamp: 0,
    };
    agent.record_obligations_from_tool_results(&[edit_result]);

    assert!(agent
        .obligation_ledger
        .contains(autonomy::ObligationKind::EditedFilesVerification));
    let (prompt, reason) = agent
        .obligation_ledger
        .next_continue()
        .expect("edit should create verification obligation");
    assert_eq!(reason, ContinueReason::ExecutionDebt);
    assert!(prompt.contains("run the narrowest relevant verification"));

    let check_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_check".to_string(),
        tool_name: "bash".to_string(),
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "exit_code": 0,
            "command": "cargo check -p imp-core"
        }),
        timestamp: 0,
    };
    agent.record_obligations_from_tool_results(&[check_result]);

    assert!(!agent
        .obligation_ledger
        .contains(autonomy::ObligationKind::EditedFilesVerification));
}

#[tokio::test]
async fn ask_user_answer_is_sent_back_to_model() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_ask",
            "ask_user",
            serde_json::json!({"question": "Which color?"}),
            100,
            20,
        ),
        text_response("You chose blue.", 120, 15),
    ]));
    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(crate::tools::ask::AskTool));
    agent.ui = Arc::new(AnsweringUi {
        answer: "blue".to_string(),
    });

    agent.run("Ask me a color".to_string()).await.unwrap();

    assert!(agent.messages.iter().any(|message| {
        matches!(
            message,
            Message::ToolResult(result)
                if result.tool_name == "ask_user"
                    && result
                        .content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Text { text } if text == "blue"))
        )
    }));
    assert!(agent.messages.iter().any(|message| {
        matches!(
            message,
            Message::Assistant(assistant)
                if assistant_message_text(assistant).contains("You chose blue.")
        )
    }));
}

#[test]
fn tool_results_indicate_execution_blocker_does_not_stop_on_ask_tool_answer() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_ask".to_string(),
        tool_name: "ask_user".to_string(),
        content: vec![ContentBlock::Text {
            text: "blue".to_string(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    };

    assert_eq!(
        tool_results_indicate_execution_blocker(&[result], AgentMode::Full),
        None
    );
}

#[test]
fn workflow_close_is_workflow_progress_not_runtime_completion() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_workflow".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Closed task".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "action": "close",
            "unit": { "id": "1", "status": "closed" }
        }),
        timestamp: 0,
    };

    assert!(!tool_results_indicate_work_completed(
        std::slice::from_ref(&result),
        AgentMode::Full
    ));
    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert!(agent.workflow_durable_progress_for_test(&[result]));
}

#[test]
fn tool_results_indicate_work_completed_detects_edit_plus_successful_check() {
    let edit_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_edit".to_string(),
        tool_name: "edit".to_string(),
        content: vec![ContentBlock::Text {
            text: "diff output".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "path": "/tmp/file.rs"
        }),
        timestamp: 0,
    };
    let check_result = imp_llm::ToolResultMessage {
        tool_call_id: "call_check".to_string(),
        tool_name: "bash".to_string(),
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "exit_code": 0,
            "command": "cargo check -p imp-core"
        }),
        timestamp: 0,
    };

    assert!(tool_results_indicate_work_completed(
        &[edit_result, check_result],
        AgentMode::Full
    ));
}

#[test]
fn tool_results_indicate_work_completed_detects_closed_unit_details() {
    let result = imp_llm::ToolResultMessage {
        tool_call_id: "call_close".to_string(),
        tool_name: "workflow".to_string(),
        content: vec![ContentBlock::Text {
            text: "Closed unit 1".to_string(),
        }],
        is_error: false,
        details: serde_json::json!({
            "action": "close",
            "unit": {
                "id": "1",
                "title": "Test unit",
                "status": "closed"
            }
        }),
        timestamp: 0,
    };

    let agent = Agent::new(
        test_model(Arc::new(MockProvider::new(vec![]))),
        PathBuf::from("/tmp"),
    )
    .0;
    assert!(agent.workflow_durable_progress_for_test(&[result]));
}

#[test]
fn workflow_review_needs_decision_maps_to_user_blocker() {
    let review = TurnWorkflowReview {
        turn_index: 0,
        state: crate::workflow_review::WorkflowReviewState::NeedsDecision,
        scope: crate::workflow_review::WorkflowReviewScope::default(),
        anchor_unit: None,
        touched_units: Vec::new(),
        proposed_children: Vec::new(),
        material_field_changes: Vec::new(),
        notes_appended: Vec::new(),
        decision_events: Vec::new(),
        unresolved_consequential_choices: Vec::new(),
        next_question: Some("Which path should we take?".to_string()),
    };

    assert_eq!(
        {
            let mut agent = Agent::new(
                test_model(Arc::new(MockProvider::new(vec![]))),
                PathBuf::from("/tmp"),
            )
            .0;
            agent.mode = AgentMode::Planner;
            agent.workflow_review_stop_reason_for_test(&review)
        },
        Some(StopReason::UserBlocker)
    );
}

#[test]
fn workflow_review_changed_with_planner_children_maps_to_decomposition_completed() {
    let review = TurnWorkflowReview {
        turn_index: 0,
        state: crate::workflow_review::WorkflowReviewState::Changed,
        scope: crate::workflow_review::WorkflowReviewScope::default(),
        anchor_unit: None,
        touched_units: Vec::new(),
        proposed_children: vec![crate::workflow_review::TurnWorkflowProposedChild {
            unit: crate::workflow_review::WorkflowUnitRef::new(
                "28.6.1",
                "child",
                Some("job".to_string()),
            ),
            parent: crate::workflow_review::WorkflowUnitRef::new(
                "28.6",
                "parent",
                Some("epic".to_string()),
            ),
            child_kind: crate::workflow_review::WorkflowReviewUnitKind::Job,
            child_origin: crate::workflow_review::WorkflowUnitOrigin::CreatedInTurn,
        }],
        material_field_changes: Vec::new(),
        notes_appended: Vec::new(),
        decision_events: Vec::new(),
        unresolved_consequential_choices: Vec::new(),
        next_question: None,
    };

    assert_eq!(
        {
            let mut agent = Agent::new(
                test_model(Arc::new(MockProvider::new(vec![]))),
                PathBuf::from("/tmp"),
            )
            .0;
            agent.mode = AgentMode::Planner;
            agent.workflow_review_stop_reason_for_test(&review)
        },
        Some(StopReason::DecompositionCompleted)
    );
}

#[tokio::test]
async fn planner_stops_after_decomposition_is_externalized() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
        "Externalized into workflow. Plan is complete and ready for handoff.",
        100,
        20,
    )]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Planner;
    agent.set_workflow_skill_available(true);

    agent.run("Plan the rollout".to_string()).await.unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts, vec!["Plan the rollout".to_string()]);
}

#[tokio::test]
async fn planner_stops_for_user_blocker_instead_of_auto_follow_up() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
        "Blocked: I need your input on which auth direction we should choose before continuing.",
        100,
        20,
    )]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Planner;
    agent.set_workflow_skill_available(true);

    agent.run("Plan the rollout".to_string()).await.unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts, vec!["Plan the rollout".to_string()]);
}

#[tokio::test]
async fn native_work_planning_then_done_continues_to_execution_debt_follow_up() {
    let provider = Arc::new(MockProvider::new(vec![
        multi_tool_call_response(
            &[
                (
                    "call_work_1",
                    "work",
                    serde_json::json!({"action": "create", "kind": "task", "title": "Plan"}),
                ),
                (
                    "call_work_2",
                    "work",
                    serde_json::json!({"action": "update", "id": "task-1", "summary": "planned"}),
                ),
                (
                    "call_work_3",
                    "work",
                    serde_json::json!({"action": "claim", "id": "task-1"}),
                ),
                (
                    "call_work_4",
                    "work",
                    serde_json::json!({"action": "context", "id": "task-1"}),
                ),
            ],
            100,
            30,
        ),
        text_response("Done.", 120, 5),
        text_response("Done.", 130, 5),
        text_response("Done.", 140, 5),
        text_response("Done.", 150, 5),
        text_response("Done.", 160, 5),
        text_response("Done.", 170, 5),
        text_response("Done.", 180, 5),
        text_response("Done.", 190, 5),
        text_response("Done.", 200, 5),
    ]));
    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;
    agent.tools.register(Arc::new(DurableWorkTool));

    let _ = agent.run("Implement the runtime fix".to_string()).await;

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert!(
        user_texts.iter().any(|text| {
            text.contains("You have recorded or planned work")
                || text.contains("Workflow state changed")
                || text.contains("Workflow graph state changed")
        }),
        "expected execution-debt follow-up after workflow planning, got {user_texts:?}"
    );
}

#[tokio::test]
async fn agent_queues_workflow_basics_hint_for_worker_workflow_requests() {
    let provider = Arc::new(MockProvider::new(vec![
        text_response("Loaded basics skill", 100, 20),
        text_response("Done", 120, 25),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_basics_skill_available(true);
    agent.mode = AgentMode::Worker;

    agent
        .run("Check workflow status and logs for my unit".to_string())
        .await
        .unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts.len(), 1);
    assert_eq!(user_texts[0], "Check workflow status and logs for my unit");
}

#[tokio::test]
async fn agent_does_not_queue_workflow_hint_without_matching_signal() {
    let provider = Arc::new(MockProvider::new(vec![text_response("No nudge", 100, 20)]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_skill_available(true);
    agent.mode = AgentMode::Planner;

    agent
        .run("Explain how this parser works".to_string())
        .await
        .unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        user_texts,
        vec!["Explain how this parser works".to_string()]
    );
}

#[tokio::test]
async fn agent_does_not_queue_workflow_basics_hint_when_no_tools_available() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
        "Loaded basics skill",
        100,
        20,
    )]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.set_workflow_basics_skill_available(true);
    agent.mode = AgentMode::Worker;
    agent.tools.retain(|_| false);

    agent
        .run("Check workflow status and logs for my unit".to_string())
        .await
        .unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        user_texts,
        vec!["Check workflow status and logs for my unit".to_string()]
    );
}

#[tokio::test]
async fn single_text_turn_with_no_tools_exits_cleanly() {
    let provider = Arc::new(MockProvider::new(vec![text_response("SMOKE_OK", 50, 10)]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Worker;
    agent.set_workflow_basics_skill_available(true);
    agent.tools.retain(|_| false);

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent
        .run("Check workflow status and finish".to_string())
        .await;
    drop(agent);

    assert!(result.is_ok());

    let events = events_task.await.unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
    assert!(!events.iter().any(|e| matches!(
        e,
        AgentEvent::Error { error } if error.contains("Max turns exceeded")
    )));
}

#[tokio::test]
async fn agent_emits_timing_events_in_order() {
    let provider = Arc::new(MockProvider::new(vec![text_response("timed", 10, 5)]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("time this".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let timings: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Timing { timing } => Some(timing.clone()),
            _ => None,
        })
        .collect();

    assert!(timings.len() >= 7);
    assert_eq!(timings[0].stage, TimingStage::ContextAssemblyStart);
    assert_eq!(timings[1].stage, TimingStage::ContextAssemblyEnd);
    assert_eq!(timings[2].stage, TimingStage::LlmRequestStart);
    assert_eq!(timings[3].stage, TimingStage::FirstStreamEvent);
    assert_eq!(timings[4].stage, TimingStage::FirstTextDelta);
    assert!(timings
        .iter()
        .any(|timing| timing.stage == TimingStage::MessageEnd));
    assert!(timings
        .iter()
        .any(|timing| timing.stage == TimingStage::PostTurnAssessmentEnd));

    for timing in timings {
        assert_eq!(timing.turn, 0);
        if let Some(since_llm_request_start_ms) = timing.since_llm_request_start_ms {
            assert!(timing.since_turn_start_ms >= since_llm_request_start_ms);
        }
    }
}

#[tokio::test]
async fn agent_streams_message_delta_before_message_end() {
    let provider = Arc::new(MockProvider::new_results(vec![vec![
        Ok(StreamEvent::MessageStart {
            model: "test-model".to_string(),
        }),
        Ok(StreamEvent::TextDelta {
            text: "streaming".to_string(),
        }),
        Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "streaming".to_string(),
                }],
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::EndTurn,
                timestamp: 1000,
            },
        }),
    ]]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Say hi".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let text_delta_idx = events.iter().position(|event| {
        matches!(
            event,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "streaming"
        )
    });
    let turn_end_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::TurnEnd { .. }));

    assert!(text_delta_idx.is_some());
    assert!(turn_end_idx.is_some());
    assert!(text_delta_idx.unwrap() < turn_end_idx.unwrap());
}

#[tokio::test]
async fn agent_treats_provider_terminal_error_as_failure_not_blank_turn() {
    let provider = Arc::new(MockProvider::new_results(vec![vec![
        Ok(StreamEvent::MessageStart {
            model: "test-model".to_string(),
        }),
        Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![],
                usage: Some(Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                stop_reason: LlmStopReason::Error(
                    "context_length_exceeded: input too large".to_string(),
                ),
                timestamp: 1000,
            },
        }),
    ]]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent.run("Continue".to_string()).await;
    drop(agent);

    assert!(result.is_err());
    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { error }
            if error == "context_length_exceeded: input too large"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentEnd {
            status: RunFinalStatus::Failed { message },
            ..
        } if message == "context_length_exceeded: input too large"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnEnd { .. })));
}

#[tokio::test]
async fn agent_surfaces_pre_output_provider_failure_as_terminal_failure() {
    let provider = Arc::new(MockProvider::new_results(vec![vec![Err(
        imp_llm::Error::ContextTooLong {
            used: 100,
            limit: 50,
        },
    )]]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent.run("Continue".to_string()).await;
    drop(agent);

    assert!(result.is_err());
    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { error }
            if error.contains("Context too long") && error.contains("exceeds 50")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentEnd {
            status: RunFinalStatus::Failed { message },
            ..
        } if message.contains("Context too long") && message.contains("exceeds 50")
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::TurnEnd { .. })));
}

#[tokio::test]
async fn agent_retries_before_first_meaningful_event_but_not_after() {
    let provider = Arc::new(MockProvider::new_results(vec![
        vec![
            Ok(StreamEvent::MessageStart {
                model: "test-model".to_string(),
            }),
            Err(imp_llm::Error::Stream("startup failure".into())),
        ],
        vec![
            Ok(StreamEvent::MessageStart {
                model: "test-model".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "recovered".to_string(),
            }),
            Ok(StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "recovered".to_string(),
                    }],
                    usage: Some(Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            }),
        ],
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Recover".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let text_delta = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "recovered"
        )
    });
    let turn_end = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnEnd { .. }));

    assert!(text_delta.is_some());
    assert!(turn_end.is_some());
    assert!(text_delta.unwrap() < turn_end.unwrap());
}

#[tokio::test]
async fn agent_recovers_after_partial_stream_failure() {
    let provider = Arc::new(MockProvider::new_results(vec![
        vec![
            Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            }),
            Err(imp_llm::Error::Stream("mid-stream failure".into())),
        ],
        vec![
            Ok(StreamEvent::TextDelta {
                text: "recovered".to_string(),
            }),
            Ok(StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "recovered".to_string(),
                    }],
                    usage: Some(Usage::default()),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            }),
        ],
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent
        .run("Recover after partial stream failure".to_string())
        .await
        .unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let error_idx = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::Error { error }
            if error.contains("Provider stream failed after partial output")
                && error.contains("mid-stream failure")
        )
    });
    let recovered_idx = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "recovered"
        )
    });
    let failed_end = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::AgentEnd {
                status: RunFinalStatus::Failed { .. },
                ..
            }
        )
    });

    assert!(error_idx.is_some());
    assert!(recovered_idx.is_some());
    assert!(error_idx.unwrap() < recovered_idx.unwrap());
    assert!(!failed_end);
}

#[tokio::test]
async fn agent_persists_partial_output_before_in_band_stream_error() {
    let provider = Arc::new(MockProvider::new(vec![vec![
        StreamEvent::TextDelta {
            text: "partial before provider error".into(),
        },
        StreamEvent::Error {
            error: "upstream server error".into(),
        },
    ]]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent.run("Handle provider error".into()).await;
    drop(agent);

    assert!(result.is_err());
    let events = events_task.await.unwrap();
    let partial = events.iter().find_map(|event| match event {
        AgentEvent::TurnEnd { message, .. } => Some(message),
        _ => None,
    });
    let partial = partial.expect("partial assistant turn should be recorded");
    assert!(partial.content.iter().any(|block| {
        matches!(block, ContentBlock::Text { text } if text == "partial before provider error")
    }));
    assert!(!partial.content.iter().any(|block| {
        matches!(block, ContentBlock::Text { text } if text == "upstream server error")
    }));
}

#[tokio::test]
async fn agent_surfaces_error_after_repeated_partial_stream_failures() {
    let provider = Arc::new(MockProvider::new_results(vec![
        vec![
            Ok(StreamEvent::TextDelta {
                text: "partial-1".to_string(),
            }),
            Err(imp_llm::Error::Stream("mid-stream failure 1".into())),
        ],
        vec![
            Ok(StreamEvent::TextDelta {
                text: "partial-2".to_string(),
            }),
            Err(imp_llm::Error::Stream("mid-stream failure 2".into())),
        ],
        vec![
            Ok(StreamEvent::TextDelta {
                text: "partial-3".to_string(),
            }),
            Err(imp_llm::Error::Stream("mid-stream failure 3".into())),
        ],
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent
        .run("Fail after repeated stream recovery failures".to_string())
        .await;
    drop(agent);

    assert!(result.is_err());

    let events = events_task.await.unwrap();
    let text_delta = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "partial-1"
        )
    });
    let error_idx = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::Error { error }
            if error.contains("Provider stream failed after partial output")
                && error.contains("mid-stream failure")
        )
    });

    assert!(text_delta.is_some());
    assert!(error_idx.is_some());
    assert!(text_delta.unwrap() < error_idx.unwrap());

    let persisted_partials: Vec<&AssistantMessage> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnEnd { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(persisted_partials.len(), 3);
    for (index, expected) in ["partial-1", "partial-2", "partial-3"].iter().enumerate() {
        assert!(persisted_partials[index]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text == expected)));
    }
}

#[tokio::test]
async fn agent_recovers_after_silent_eof_without_message_end() {
    let provider = Arc::new(MockProvider::new_results(vec![
        vec![Ok(StreamEvent::TextDelta {
            text: "partial".to_string(),
        })],
        vec![
            Ok(StreamEvent::TextDelta {
                text: "recovered".to_string(),
            }),
            Ok(StreamEvent::MessageEnd {
                message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "recovered".to_string(),
                    }],
                    usage: Some(Usage::default()),
                    stop_reason: LlmStopReason::EndTurn,
                    timestamp: 1000,
                },
            }),
        ],
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent
        .run("Recover from silent eof".to_string())
        .await
        .unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let text_delta = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "partial"
        )
    });
    let error_idx = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::Error { error }
            if error.contains("missing terminal completion event")
        )
    });
    let recovered_idx = events.iter().position(|e| {
        matches!(
            e,
            AgentEvent::MessageDelta {
                delta: StreamEvent::TextDelta { text }
            } if text == "recovered"
        )
    });

    assert!(text_delta.is_some());
    assert!(error_idx.is_some());
    assert!(recovered_idx.is_some());
    assert!(text_delta.unwrap() < error_idx.unwrap());
    assert!(error_idx.unwrap() < recovered_idx.unwrap());
}

// ── Test 1: Simple text response ───────────────────────────────

#[tokio::test]
async fn agent_simple_text_response() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
        "Hello, world!",
        100,
        20,
    )]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Say hello".to_string()).await.unwrap();
    drop(agent); // close event channel

    let events = events_task.await.unwrap();

    // Verify event order: AgentStart → TurnStart → deltas → TurnEnd → AgentEnd
    assert!(matches!(events[0], AgentEvent::AgentStart { .. }));

    let turn_start = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnStart { index: 0 }));
    assert!(turn_start.is_some());

    let turn_end = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnEnd { index: 0, .. }));
    assert!(turn_end.is_some());
    assert!(turn_end.unwrap() > turn_start.unwrap());

    let agent_end = events
        .iter()
        .position(|e| matches!(e, AgentEvent::AgentEnd { .. }));
    assert!(agent_end.is_some());
    assert!(agent_end.unwrap() > turn_end.unwrap());

    // Verify usage
    if let AgentEvent::AgentEnd { usage, cost, .. } = &events[agent_end.unwrap()] {
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert!(cost.total > 0.0);
    } else {
        panic!("Expected AgentEnd");
    }

    // Only one turn (no tool calls)
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert_eq!(turn_starts.len(), 1);
}

// ── Test 2: Single tool call → result → text response ──────────

#[tokio::test]
async fn agent_single_tool_call() {
    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: model calls echo tool
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            30,
        ),
        // Turn 1: model responds with text after seeing tool result
        text_response("The echo said: hello", 200, 25),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo hello".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // Should have 2 TurnStart events (turn 0 with tool, turn 1 with text)
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert_eq!(turn_starts.len(), 2);

    // Should have tool execution events
    let tool_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .collect();
    assert_eq!(tool_starts.len(), 1);

    let tool_ends: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        .collect();
    assert_eq!(tool_ends.len(), 1);

    // Verify accumulated usage across turns (100 + 200 input, 30 + 25 output)
    if let Some(AgentEvent::AgentEnd { usage, .. }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    {
        assert_eq!(usage.input_tokens, 300);
        assert_eq!(usage.output_tokens, 55);
    } else {
        panic!("Expected AgentEnd");
    }
}

// ── Test 3: Multiple tool calls → follow-up tool calls → done ──

#[tokio::test]
async fn agent_multiple_tool_calls() {
    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: model calls echo twice
        multi_tool_call_response(
            &[
                ("call_1", "echo", serde_json::json!({"text": "first"})),
                ("call_2", "echo", serde_json::json!({"text": "second"})),
            ],
            100,
            40,
        ),
        // Turn 1: model calls echo once more
        tool_call_response(
            "call_3",
            "echo",
            serde_json::json!({"text": "third"}),
            200,
            20,
        ),
        // Turn 2: model responds with final text
        text_response("All done!", 300, 10),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Echo three things".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // 3 turns
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert_eq!(turn_starts.len(), 3);

    // 3 tool executions total
    let tool_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .collect();
    assert_eq!(tool_starts.len(), 3);

    // Total usage: 100+200+300=600 input, 40+20+10=70 output
    if let Some(AgentEvent::AgentEnd { usage, .. }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    {
        assert_eq!(usage.input_tokens, 600);
        assert_eq!(usage.output_tokens, 70);
    } else {
        panic!("Expected AgentEnd");
    }
}

// ── Test 4: Cancel command mid-run ─────────────────────────────

#[tokio::test]
async fn execution_does_not_stop_after_work_completed_text() {
    let provider = Arc::new(MockProvider::new(vec![
        text_response(
            "All done! Implemented the change and finished the task.",
            100,
            20,
        ),
        text_response("No further runtime evidence is available.", 100, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;

    agent.run("Implement the change".to_string()).await.unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts, vec!["Implement the change".to_string()]);
    assert_eq!(
        agent
            .assess_post_turn(
                agent
                    .messages
                    .iter()
                    .rev()
                    .find_map(|message| match message {
                        Message::Assistant(assistant) => Some(assistant),
                        _ => None,
                    })
                    .unwrap(),
                &[],
                false,
                &TurnWorkflowReview::no_change(0),
            )
            .text_fallback
            .execution_stop_reason,
        None
    );
}

#[tokio::test]
async fn execution_does_not_stop_for_user_blocker_text() {
    let provider = Arc::new(MockProvider::new(vec![
        text_response(
            "Blocked: I need your input on which path to take before continuing.",
            100,
            20,
        ),
        text_response("No further runtime evidence is available.", 100, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.mode = AgentMode::Full;

    agent.run("Implement the change".to_string()).await.unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(user_texts, vec!["Implement the change".to_string()]);
    assert_eq!(
        agent
            .assess_post_turn(
                agent
                    .messages
                    .iter()
                    .rev()
                    .find_map(|message| match message {
                        Message::Assistant(assistant) => Some(assistant),
                        _ => None,
                    })
                    .unwrap(),
                &[],
                false,
                &TurnWorkflowReview::no_change(0),
            )
            .text_fallback
            .execution_stop_reason,
        None
    );
}

#[tokio::test]
async fn agent_follow_up_runs_after_current_work_finishes() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            20,
        ),
        text_response("Handled follow-up", 120, 25),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    handle
        .command_tx
        .send(AgentCommand::FollowUp("What next?".into()))
        .await
        .unwrap();

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Do the first thing".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert_eq!(turn_starts.len(), 2);
}

#[tokio::test]
async fn agent_follow_up_preserves_order_with_multiple_messages() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            20,
        ),
        text_response("First follow-up handled", 120, 25),
        text_response("Second follow-up handled", 130, 30),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    handle
        .command_tx
        .send(AgentCommand::FollowUp("follow up one".into()))
        .await
        .unwrap();
    handle
        .command_tx
        .send(AgentCommand::FollowUp("follow up two".into()))
        .await
        .unwrap();

    agent.run("Do the first thing".to_string()).await.unwrap();

    let user_texts: Vec<String> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    assert_eq!(
        user_texts,
        vec![
            "Do the first thing".to_string(),
            "follow up one".to_string(),
            "follow up two".to_string()
        ]
    );
}

#[tokio::test]
async fn agent_cancel_still_wins_over_follow_up_queue() {
    let provider = Arc::new(MockProvider::new(vec![tool_call_response(
        "call_1",
        "echo",
        serde_json::json!({"text": "hello"}),
        100,
        20,
    )]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    handle
        .command_tx
        .send(AgentCommand::FollowUp("queued later".into()))
        .await
        .unwrap();
    handle.command_tx.send(AgentCommand::Cancel).await.unwrap();

    let result = agent.run("Do something".to_string()).await;
    assert!(matches!(result, Err(crate::error::Error::Cancelled)));
}

#[tokio::test]
async fn agent_allows_non_workflow_bash_commands() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "bash",
            serde_json::json!({"command": "printf 'ok'", "timeout": 5}),
            100,
            20,
        ),
        text_response("done", 120, 25),
    ]));

    let model = test_model(provider);
    let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(crate::tools::bash::BashTool));

    agent.run("Run a shell command".to_string()).await.unwrap();

    let tool_result = agent
        .messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .expect("expected tool result");
    assert!(!tool_result.is_error);
}

#[tokio::test]
async fn agent_cancel_mid_run() {
    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: tool call (agent will process this, then see Cancel before turn 1)
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            100,
            20,
        ),
        // Turn 1: this should never be reached
        text_response("Should not see this", 100, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    // Send cancel before the second turn
    handle.command_tx.send(AgentCommand::Cancel).await.unwrap();

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent.run("Do something".to_string()).await;
    drop(agent);

    // Should return Cancelled error
    assert!(matches!(result, Err(crate::error::Error::Cancelled)));

    let events = events_task.await.unwrap();

    // Should have AgentEnd
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));

    // Should NOT have a second turn
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert!(turn_starts.len() <= 1);
}

#[tokio::test]
async fn single_text_turn_exits_cleanly() {
    let provider = Arc::new(MockProvider::new(vec![text_response("SMOKE_OK", 50, 10)]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));

    let events_task = tokio::spawn(collect_events(handle));
    let result = agent.run("Reply once and stop".to_string()).await;
    drop(agent);

    assert!(result.is_ok());

    let events = events_task.await.unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
    assert!(!events.iter().any(|e| matches!(
        e,
        AgentEvent::Error { error } if error.contains("Max turns exceeded")
    )));
}

// ── Test 6: Unknown tool → error result → model self-corrects ──

#[tokio::test]
async fn agent_unknown_tool_self_corrects() {
    let provider = Arc::new(MockProvider::new(vec![
        // Turn 0: model calls a tool that doesn't exist
        tool_call_response(
            "call_1",
            "nonexistent",
            serde_json::json!({"foo": "bar"}),
            100,
            20,
        ),
        // Turn 1: model self-corrects and responds with text
        text_response("Sorry, I used the wrong tool. Here's the answer.", 200, 30),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    // Deliberately NOT registering the "nonexistent" tool

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Do something".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // The tool execution should produce an error result
    let tool_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }));
    assert!(tool_end.is_some());
    if let Some(AgentEvent::ToolExecutionEnd { result, .. }) = tool_end {
        assert!(result.is_error);
        let text = result.content.iter().find_map(|c| {
            if let ContentBlock::Text { text } = c {
                Some(text.as_str())
            } else {
                None
            }
        });
        assert!(text.unwrap().contains("Unknown tool"));
    }

    // Model should have self-corrected in turn 1
    let turn_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .collect();
    assert_eq!(turn_starts.len(), 2);

    // Should complete successfully
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

#[tokio::test]
async fn agent_concurrent_readonly() {
    let shared = Arc::new(ConcurrentReadonlyState::new(3));
    let provider = Arc::new(MockProvider::new(vec![
        multi_tool_call_response(
            &[
                ("call_ro_1", "echo_a", serde_json::json!({"text": "first"})),
                (
                    "call_write",
                    "write_after_reads",
                    serde_json::json!({"data": "mutate"}),
                ),
                ("call_ro_2", "echo_b", serde_json::json!({"text": "second"})),
                ("call_ro_3", "echo_c", serde_json::json!({"text": "third"})),
            ],
            100,
            40,
        ),
        text_response("All tools finished", 150, 20),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    drop(handle);

    agent.tools.register(Arc::new(CoordinatedReadonlyTool {
        name: "echo_a",
        shared: shared.clone(),
    }));
    agent.tools.register(Arc::new(CoordinatedReadonlyTool {
        name: "echo_b",
        shared: shared.clone(),
    }));
    agent.tools.register(Arc::new(CoordinatedReadonlyTool {
        name: "echo_c",
        shared: shared.clone(),
    }));
    agent.tools.register(Arc::new(CoordinatedMutableTool {
        shared: shared.clone(),
    }));

    tokio::time::timeout(
        Duration::from_millis(250),
        agent.run("Run all tools".to_string()),
    )
    .await
    .expect("read-only tools should not block each other")
    .expect("agent should complete successfully");

    let tool_result_ids: Vec<_> = agent
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_result_ids,
        vec!["call_ro_1", "call_write", "call_ro_2", "call_ro_3"]
    );

    assert_eq!(shared.readonly_started.load(Ordering::SeqCst), 3);
    assert_eq!(shared.readonly_finished.load(Ordering::SeqCst), 3);
    assert_eq!(shared.mutable_observed_finished.load(Ordering::SeqCst), 3);

    let log = shared.log.lock().expect("concurrent log lock").clone();
    assert_eq!(
        log.last().map(String::as_str),
        Some("write_after_reads:start")
    );
}

// ── Event ordering validation ──────────────────────────────────

#[tokio::test]
async fn agent_event_ordering() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "hello"}),
            50,
            10,
        ),
        text_response("Done", 50, 10),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("test".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // Extract event types in order
    let types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            AgentEvent::AgentStart { .. } => "AgentStart",
            AgentEvent::AgentEnd { .. } => "AgentEnd",
            AgentEvent::TurnStart { .. } => "TurnStart",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::MessageDelta { .. } => "MessageDelta",
            AgentEvent::ToolExecutionStart { .. } => "ToolExecStart",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecEnd",
            AgentEvent::Warning { .. } => "Warning",
            AgentEvent::EvidenceWritten { .. } => "EvidenceWritten",
            AgentEvent::VerificationStarted { .. } => "VerificationStarted",
            AgentEvent::VerificationCompleted { .. } => "VerificationCompleted",
            AgentEvent::PolicyChecked { .. } => "PolicyChecked",
            AgentEvent::Error { .. } => "Error",
            _ => "Other",
        })
        .collect();

    // Must start with AgentStart
    assert_eq!(types[0], "AgentStart");

    // Must end with AgentEnd
    assert_eq!(types[types.len() - 1], "AgentEnd");

    // TurnStart must come before TurnEnd for each turn
    let mut turn_start_indices: Vec<usize> = Vec::new();
    let mut turn_end_indices: Vec<usize> = Vec::new();
    for (i, t) in types.iter().enumerate() {
        if *t == "TurnStart" {
            turn_start_indices.push(i);
        }
        if *t == "TurnEnd" {
            turn_end_indices.push(i);
        }
    }
    assert_eq!(turn_start_indices.len(), 2);
    assert_eq!(turn_end_indices.len(), 2);
    for i in 0..turn_start_indices.len() {
        assert!(turn_start_indices[i] < turn_end_indices[i]);
    }

    // ToolExecStart must come before ToolExecEnd
    let tool_start = types.iter().position(|t| *t == "ToolExecStart");
    let tool_end = types.iter().position(|t| *t == "ToolExecEnd");
    assert!(tool_start.is_some());
    assert!(tool_end.is_some());
    assert!(tool_start.unwrap() < tool_end.unwrap());
}

#[tokio::test]
async fn agent_fires_hooks() {
    let provider = Arc::new(MockProvider::new(vec![text_response("hooked", 100, 20)]));
    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    drop(handle);

    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls_for_callback = hook_calls.clone();
    agent.hooks.register(crate::hooks::HookDefinition {
        event: "before_llm_call".to_string(),
        match_pattern: None,
        action: crate::hooks::HookAction::Callback(Arc::new(move |_event| {
            hook_calls_for_callback.fetch_add(1, Ordering::SeqCst);
            crate::hooks::HookResult::default()
        })),
        blocking: true,
        threshold: None,
    });

    agent.run("Run once".to_string()).await.unwrap();

    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn agent_context_masking() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));

    let mut seeded_messages = Vec::new();
    for index in 0..12 {
        let call_id = format!("call_{index}");
        seeded_messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/file_{index}.rs")}),
        ));
        seeded_messages.push(make_tool_result(&call_id, "read", &"x".repeat(400)));
    }

    let mut usage_messages = seeded_messages.clone();
    usage_messages.push(Message::user("trigger masking"));
    let provisional_model = test_model(provider.clone());
    let usage = crate::context::context_usage(&usage_messages, &provisional_model);
    let context_window = ((usage.used as f64) / 0.7).ceil() as u32;

    let model = test_model_with_context_window(provider.clone(), context_window.max(1));
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    drop(handle);
    agent.messages = seeded_messages;

    agent.run("trigger masking".to_string()).await.unwrap();

    let canonical = tool_result_text(&agent.messages[1]).expect("first tool result text");
    let expected_old = "x".repeat(400);
    assert_eq!(canonical, expected_old.as_str());

    let contexts = provider.contexts();
    let provider_context = contexts.first().expect("provider context");
    let masked = tool_result_text(&provider_context.messages[1]).expect("first tool result text");
    assert!(masked.starts_with("[Output omitted"));

    let recent_index = (10 * 2) + 1;
    let recent = tool_result_text(&provider_context.messages[recent_index])
        .expect("recent tool result text");
    let expected_recent = "x".repeat(400);
    assert_eq!(recent, expected_recent.as_str());
}

#[tokio::test]
async fn agent_auto_compacts_tool_heavy_context_before_provider_request() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));
    let huge_output = "x".repeat(80_000);

    let mut seeded_messages = Vec::new();
    for index in 0..8 {
        let call_id = format!("call_{index}");
        seeded_messages.push(Message::user(format!("inspect src/file_{index}.rs")));
        seeded_messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/file_{index}.rs")}),
        ));
        seeded_messages.push(make_tool_result(&call_id, "read", &huge_output));
    }

    let mut usage_messages = seeded_messages.clone();
    usage_messages.push(Message::user("continue"));
    let provisional_model = test_model(provider.clone());
    let usage = crate::context::context_usage(&usage_messages, &provisional_model);
    let context_window = ((usage.used as f64) / 0.5).ceil() as u32;

    let model = test_model_with_context_window(provider, context_window.max(1));
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.context_config.observation_mask_threshold = 2.0;
    agent.context_config.auto_compaction.mode = crate::config::AutoCompactionMode::NearThreshold;
    agent.context_config.auto_compaction.trigger_ratio = 0.40;
    agent.messages = seeded_messages;

    agent.run("continue".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Warning { message }
            if message.contains("Automatic compaction threshold reached without an activated durable checkpoint")
    )));
}

#[tokio::test]
async fn auto_compaction_does_not_run_at_moderate_nominal_usage() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));
    let model = large_window_test_model(provider);
    let seeded_messages = tool_heavy_messages_for_usage(&model, 270_000);

    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.context_config.observation_mask_threshold = 2.0;
    agent.context_config.auto_compaction.mode = crate::config::AutoCompactionMode::NearThreshold;
    agent.context_config.auto_compaction.trigger_ratio = 0.85;
    agent.messages = seeded_messages;

    agent.run("continue".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    assert!(events.iter().all(|event| !matches!(
        event,
        AgentEvent::Warning { message }
            if message.contains("Automatic compaction threshold reached without an activated durable checkpoint")
    )));
}

#[tokio::test]
async fn observed_provider_ceiling_lowers_auto_compaction_threshold() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));
    let model = large_window_test_model(provider);
    let seeded_messages = tool_heavy_messages_for_usage(&model, 270_000);

    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.context_config.observation_mask_threshold = 2.0;
    agent.context_config.auto_compaction.mode = crate::config::AutoCompactionMode::NearThreshold;
    agent.context_config.auto_compaction.trigger_ratio = 0.85;
    agent.context_config.auto_compaction.target_ratio = 0.70;
    agent.observed_context_input_limit = Some(300_000);
    agent.messages = seeded_messages;

    agent.run("continue".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Warning { message }
            if message.contains("Automatic compaction threshold reached without an activated durable checkpoint")
    )));
}

#[tokio::test]
async fn auto_compaction_preserves_latest_tool_call_result_pair() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));
    let model = large_window_test_model(provider);
    let mut seeded_messages = tool_heavy_messages_for_usage(&model, 270_000);
    let latest_call_id = "latest_call_to_preserve";
    seeded_messages.push(make_assistant_tool_call(
        latest_call_id,
        "read",
        serde_json::json!({"path": "src/current.rs"}),
    ));
    seeded_messages.push(make_tool_result(
        latest_call_id,
        "read",
        "current file content that must remain paired with its tool call",
    ));

    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.context_config.observation_mask_threshold = 2.0;
    agent.context_config.auto_compaction.mode = crate::config::AutoCompactionMode::NearThreshold;
    agent.context_config.auto_compaction.trigger_ratio = 0.85;
    agent.context_config.auto_compaction.target_ratio = 0.70;
    agent.observed_context_input_limit = Some(300_000);
    agent.messages = seeded_messages;

    agent.run("continue".to_string()).await.unwrap();

    let has_latest_call = agent.messages.iter().any(|message| match message {
        Message::Assistant(assistant) => assistant.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolCall { id, .. } if id == latest_call_id
            )
        }),
        _ => false,
    });
    let has_latest_result = agent.messages.iter().any(|message| {
        matches!(
            message,
            Message::ToolResult(result) if result.tool_call_id == latest_call_id
        )
    });
    drop(agent);

    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Warning { message }
            if message.contains("Automatic compaction threshold reached without an activated durable checkpoint")
    )));
    assert!(
        has_latest_call,
        "latest assistant tool call was compacted away"
    );
    assert!(has_latest_result, "latest tool result was compacted away");
}

#[tokio::test]
async fn agent_masks_observations_when_context_is_tight() {
    let provider = Arc::new(MockProvider::new(vec![text_response("done", 100, 20)]));

    let mut seeded_messages = Vec::new();
    for index in 0..12 {
        let call_id = format!("call_{index}");
        seeded_messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/file_{index}.rs")}),
        ));
        seeded_messages.push(make_tool_result(&call_id, "read", &"x".repeat(400)));
    }

    let mut usage_messages = seeded_messages.clone();
    usage_messages.push(Message::user("trigger masking"));
    let provisional_model = test_model(provider.clone());
    let usage_before = crate::context::context_usage(&usage_messages, &provisional_model);

    let mut masked_messages = usage_messages.clone();
    crate::context::mask_observations(&mut masked_messages, 10);
    let usage_after = crate::context::context_usage(&masked_messages, &provisional_model);

    assert!(usage_before.used > usage_after.used);

    // Pick a window where masking definitely triggers.
    let context_window = ((usage_before.used as f64) / 0.7).ceil() as u32;

    let model = test_model_with_context_window(provider, context_window.max(1));
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.messages = seeded_messages;

    agent.run("trigger masking".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStart { index: 0 })),
        "agent should still run normally"
    );
}

#[tokio::test]
async fn agent_reports_context_full_before_provider_request() {
    let provider = Arc::new(MockProvider::new(vec![text_response(
        "should not be called",
        1,
        1,
    )]));
    let model = test_model_with_context_window(provider, 1);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));

    let result = agent
        .run("this message is definitely too large".to_string())
        .await;
    drop(agent);

    assert!(matches!(
        result,
        Err(crate::error::Error::Llm(
            imp_llm::Error::ContextTooLong { .. }
        ))
    ));

    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { error }
            if error.contains("Context full") && error.contains("Run /compact")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::AgentEnd {
            status: RunFinalStatus::Failed { message },
            ..
        } if message.contains("Context full")
    )));
}

#[tokio::test]
async fn agent_recovers_when_provider_returns_context_error_message_end() {
    let provider = Arc::new(MockProvider::new(vec![
        context_length_exceeded_response(),
        text_response("recovered", 100, 20),
    ]));

    let mut seeded_messages = Vec::new();
    for index in 0..12 {
        let call_id = format!("call_{index}");
        seeded_messages.push(make_assistant_tool_call(
            &call_id,
            "read",
            serde_json::json!({"path": format!("src/file_{index}.rs")}),
        ));
        seeded_messages.push(make_tool_result(&call_id, "read", &"x".repeat(400)));
    }

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    let events_task = tokio::spawn(collect_events(handle));
    agent.context_config.observation_mask_threshold = 2.0;
    agent.messages = seeded_messages;

    agent.run("continue".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Warning { message }
            if message.contains("context exhaustion after completing an error response")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ContextUsageUpdated {
            used,
            display_window,
            ..
        }
            if used == display_window
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnEnd { message, .. }
            if message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text == "recovered"))
    )));
}

// ── Usage/cost accumulation ────────────────────────────────────

#[tokio::test]
async fn agent_usage_cost_accumulation() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_call_response(
            "call_1",
            "echo",
            serde_json::json!({"text": "a"}),
            1_000_000, // 1M input tokens
            500_000,   // 500k output tokens
        ),
        text_response("done", 1_000_000, 500_000),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.tools.register(Arc::new(EchoTool));

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("test".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    if let Some(AgentEvent::AgentEnd { usage, cost, .. }) = events
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
    {
        // 2M input, 1M output
        assert_eq!(usage.input_tokens, 2_000_000);
        assert_eq!(usage.output_tokens, 1_000_000);

        // Cost: 2M * $3/Mtok input = $6, 1M * $15/Mtok output = $15, total = $21
        assert!((cost.input - 6.0).abs() < 1e-10);
        assert!((cost.output - 15.0).abs() < 1e-10);
        assert!((cost.total - 21.0).abs() < 1e-10);
    } else {
        panic!("Expected AgentEnd");
    }
}

// ── Retry policy tests ─────────────────────────────────────────

/// A mock provider that returns a fixed sequence of results. Each call to
/// `stream()` returns the next item: an `Err` for errors, or a pre-built
/// event sequence for success.
struct RetryMockProvider {
    calls: Mutex<Vec<std::result::Result<Vec<StreamEvent>, imp_llm::Error>>>,
}

impl RetryMockProvider {
    fn new(calls: Vec<std::result::Result<Vec<StreamEvent>, imp_llm::Error>>) -> Self {
        Self {
            calls: Mutex::new(calls),
        }
    }
}

#[async_trait]
impl Provider for RetryMockProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
        let mut calls = self.calls.try_lock().expect("RetryMockProvider lock");
        let outcome = if calls.is_empty() {
            Ok(vec![StreamEvent::Error {
                error: "No more mock responses".to_string(),
            }])
        } else {
            calls.remove(0)
        };
        match outcome {
            Ok(events) => Box::pin(futures::stream::iter(
                events.into_iter().map(imp_llm::Result::Ok),
            )),
            Err(e) => Box::pin(futures::stream::once(async move {
                imp_llm::Result::<StreamEvent>::Err(e)
            })),
        }
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok("mock-key".to_string())
    }

    fn id(&self) -> &str {
        "retry-mock"
    }

    fn models(&self) -> &[ModelMeta] {
        &[]
    }
}

/// Provider that fails N times with a rate-limit error, then succeeds.
#[tokio::test]
async fn retry_succeeds_after_transient_failures() {
    use imp_llm::provider::RetryPolicy;

    let provider = Arc::new(RetryMockProvider::new(vec![
        // First two calls fail with a rate-limit error
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
        // Third call succeeds
        Ok(text_response("Hello after retries", 100, 20)),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    // Zero delays so the test runs fast
    agent.retry_policy = RetryPolicy {
        max_retries: 3,
        base_delay: std::time::Duration::from_millis(0),
        max_delay: std::time::Duration::from_secs(30),
        retry_on: vec![],
    };

    let events_task = tokio::spawn(collect_events(handle));
    agent.run("Say hello".to_string()).await.unwrap();
    drop(agent);

    let events = events_task.await.unwrap();

    // Agent should have completed successfully
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));

    // The final text should be present in TurnEnd
    let turn_end = events.iter().find_map(|e| match e {
        AgentEvent::TurnEnd { message, .. } => Some(message),
        _ => None,
    });
    assert!(turn_end.is_some());
    let content_text = turn_end
        .unwrap()
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");
    assert!(
        content_text.contains("Hello after retries"),
        "expected final text, got: {content_text}"
    );
}

/// When max_retries is exhausted the agent returns an error.
#[tokio::test]
async fn retry_fails_when_max_retries_exhausted() {
    use imp_llm::provider::RetryPolicy;

    let provider = Arc::new(RetryMockProvider::new(vec![
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
        Err(imp_llm::Error::RateLimited {
            retry_after_secs: Some(0),
        }),
    ]));

    let model = test_model(provider);
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.retry_policy = RetryPolicy {
        max_retries: 2, // only 2 retries allowed
        base_delay: std::time::Duration::from_millis(0),
        max_delay: std::time::Duration::from_secs(30),
        retry_on: vec![],
    };
    drop(handle);

    let result = agent.run("Fail".to_string()).await;
    assert!(
        result.is_err(),
        "should have failed after exhausting retries"
    );
}

/// Auth errors (HTTP 401/403) must NOT be retried.
#[tokio::test]
async fn retry_does_not_retry_auth_errors() {
    use imp_llm::provider::RetryPolicy;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    struct CountingAuthFailProvider {
        calls: AtomicUsize,
        success_after: usize,
    }

    #[async_trait]
    impl Provider for CountingAuthFailProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.success_after {
                Box::pin(futures::stream::once(async {
                    Err(imp_llm::Error::Auth("Invalid API key".to_string()))
                }))
            } else {
                Box::pin(futures::stream::iter(
                    text_response("ok", 10, 5).into_iter().map(Ok),
                ))
            }
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("mock-key".to_string())
        }

        fn id(&self) -> &str {
            "auth-fail-mock"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    let _ = call_count_clone; // silence unused warning

    let provider = Arc::new(CountingAuthFailProvider {
        calls: AtomicUsize::new(0),
        success_after: 999, // would succeed eventually, but we expect no retry
    });
    let call_ref = &provider.calls;

    let model = test_model(provider.clone());
    let (mut agent, handle) = Agent::new(model, PathBuf::from("/tmp"));
    agent.retry_policy = RetryPolicy {
        max_retries: 5, // generous, to confirm auth errors bypass retry entirely
        base_delay: std::time::Duration::from_millis(0),
        max_delay: std::time::Duration::from_secs(30),
        retry_on: vec![],
    };
    drop(handle);

    let result = agent.run("Auth test".to_string()).await;
    assert!(result.is_err(), "should fail on auth error");

    // The provider should have been called exactly once — no retries.
    assert_eq!(
        call_ref.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "auth errors should not be retried"
    );
}
