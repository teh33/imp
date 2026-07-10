use super::*;
use imp_llm::{
    auth::{ApiKey, AuthStore},
    model::{Capabilities, ModelPricing},
    provider::{Context, Provider, RequestOptions},
    AssistantMessage, ContentBlock, ModelMeta, StopReason, StreamEvent, Usage,
};
use serde_json::json;
use tempfile::TempDir;

struct NoopProvider {
    models: Vec<ModelMeta>,
}

struct SingleResponseProvider {
    models: Vec<ModelMeta>,
    events: std::sync::Mutex<Option<Vec<imp_llm::Result<StreamEvent>>>>,
}

#[async_trait::async_trait]
impl Provider for NoopProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> std::pin::Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>
    {
        Box::pin(futures::stream::empty())
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok(String::new())
    }

    fn id(&self) -> &str {
        "noop"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

#[async_trait::async_trait]
impl Provider for SingleResponseProvider {
    fn stream(
        &self,
        _model: &Model,
        _context: Context,
        _options: RequestOptions,
        _api_key: &str,
    ) -> std::pin::Pin<Box<dyn futures_core::Stream<Item = imp_llm::Result<StreamEvent>> + Send>>
    {
        let events = self
            .events
            .lock()
            .expect("single response provider lock")
            .take()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(events))
    }

    async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
        Ok(String::new())
    }

    fn id(&self) -> &str {
        "single-response"
    }

    fn models(&self) -> &[ModelMeta] {
        &self.models
    }
}

fn test_model() -> Model {
    let meta = ModelMeta {
        id: "test-model".into(),
        provider: "test-provider".into(),
        name: "Test Model".into(),
        context_window: 8192,
        max_output_tokens: 2048,
        pricing: ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 4.0,
            cache_read_per_mtok: 0.5,
            cache_write_per_mtok: 1.0,
        },
        capabilities: Capabilities {
            reasoning: false,
            images: false,
            tool_use: true,
        },
    };
    Model {
        meta: meta.clone(),
        provider: Arc::new(NoopProvider { models: vec![meta] }),
    }
}

fn test_model_with_events(events: Vec<imp_llm::Result<StreamEvent>>) -> Model {
    let meta = ModelMeta {
        id: "test-model".into(),
        provider: "test-provider".into(),
        name: "Test Model".into(),
        context_window: 8192,
        max_output_tokens: 2048,
        pricing: ModelPricing {
            input_per_mtok: 2.0,
            output_per_mtok: 4.0,
            cache_read_per_mtok: 0.5,
            cache_write_per_mtok: 1.0,
        },
        capabilities: Capabilities {
            reasoning: false,
            images: false,
            tool_use: true,
        },
    };
    Model {
        meta: meta.clone(),
        provider: Arc::new(SingleResponseProvider {
            models: vec![meta],
            events: std::sync::Mutex::new(Some(events)),
        }),
    }
}

fn test_assistant_message(timestamp: u64, usage: Option<Usage>) -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "done".into(),
        }],
        usage,
        stop_reason: StopReason::EndTurn,
        timestamp,
    }
}

#[test]
fn session_options_default_is_sensible() {
    let opts = SessionOptions::default();
    assert!(opts.model.is_none());
    assert!(opts.max_tokens.is_none());
    assert!(!opts.no_tools);
    assert!(matches!(opts.session, SessionChoice::New));
}

#[test]
fn resolve_runtime_connection_prefers_openai_chatgpt_route_when_oauth_exists() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let mut auth_store = AuthStore::new(auth_path);
    auth_store
        .store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("gpt-5.4"),
            config_model: None,
            provider_override: Some("openai"),
            api_key_override_present: false,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.model_id, "gpt-5.4");
    assert_eq!(resolved.provider_name, "openai-codex");
}

#[test]
fn resolve_runtime_connection_respects_forced_non_openai_provider() {
    let auth_path = PathBuf::from("/tmp/nonexistent-auth.json");
    let auth_store = AuthStore::new(auth_path);
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("gpt-5.4"),
            config_model: None,
            provider_override: Some("anthropic"),
            api_key_override_present: false,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.provider_name, "anthropic");
}

#[test]
fn resolve_runtime_connection_does_not_switch_when_model_is_not_codex_supported() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let mut auth_store = AuthStore::new(auth_path);
    auth_store
        .store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("gpt-4o"),
            config_model: None,
            provider_override: Some("openai"),
            api_key_override_present: false,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.model_id, "gpt-4o");
    assert_eq!(resolved.provider_name, "openai");
}

#[test]
fn resolve_runtime_connection_does_not_switch_when_api_key_override_is_present() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let mut auth_store = AuthStore::new(auth_path);
    auth_store
        .store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("gpt-5.4"),
            config_model: None,
            provider_override: None,
            api_key_override_present: true,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.model_id, "gpt-5.4");
    assert_eq!(resolved.provider_name, "openai");
}

#[test]
fn resolve_runtime_connection_prefers_kimi_code_route_when_oauth_exists_without_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let mut auth_store = AuthStore::new(auth_path);
    auth_store
        .store(
            "kimi-code",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("kimi"),
            config_model: None,
            provider_override: None,
            api_key_override_present: false,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.model_id, "kimi2.6");
    assert_eq!(resolved.provider_name, "kimi-code");
}

#[test]
fn resolve_runtime_connection_keeps_moonshot_kimi_when_api_key_exists() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let mut auth_store = AuthStore::new(auth_path);
    auth_store
        .store(
            "moonshot",
            imp_llm::auth::StoredCredential::ApiKey {
                key: "sk-moonshot".into(),
            },
        )
        .unwrap();
    auth_store
        .store(
            "kimi-code",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();
    let registry = ModelRegistry::with_builtins();

    let resolved = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: Some("kimi"),
            config_model: None,
            provider_override: None,
            api_key_override_present: false,
        },
        &auth_store,
        &registry,
    )
    .unwrap();

    assert_eq!(resolved.model_id, "kimi-k2.6");
    assert_eq!(resolved.provider_name, "moonshot");
}

#[tokio::test]
async fn no_tools_session_surfaces_auth_failure_instead_of_empty_api_key() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let auth_path = tmp.path().join("auth.json");
    std::fs::create_dir_all(&cwd).unwrap();

    let result = ImpSession::create(SessionOptions {
        cwd: cwd.clone(),
        auth_path: Some(auth_path),
        provider: Some("openai-codex".into()),
        model: Some("gpt-5.4".into()),
        no_tools: true,
        session: SessionChoice::InMemory,
        ..Default::default()
    })
    .await;

    match result {
        Ok(_) => panic!("missing auth should fail clearly"),
        Err(Error::Config(message)) => {
            assert!(message.contains("Auth failed for openai-codex"));
            assert!(!message.contains("Incorrect API key provided: ''"));
        }
        Err(other) => panic!("expected config error, got {other:?}"),
    }
}

#[tokio::test]
async fn no_tools_session_builds_assembled_system_prompt_when_task_present() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let auth_path = tmp.path().join("auth.json");
    std::fs::create_dir_all(&cwd).unwrap();

    let mut auth_store = AuthStore::new(auth_path.clone());
    auth_store
        .store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(imp_llm::auth::OAuthCredential {
                access_token: "oauth-token".into(),
                refresh_token: "refresh-token".into(),
                expires_at: imp_llm::now() + 3600,
            }),
        )
        .unwrap();

    let session = ImpSession::create(SessionOptions {
        cwd: cwd.clone(),
        auth_path: Some(auth_path),
        provider: Some("openai".into()),
        model: Some("gpt-5.4".into()),
        no_tools: true,
        session: SessionChoice::InMemory,
        task: Some(TaskContext {
            title: "Test task".into(),
            description: "Verify headless prompt assembly".into(),
            design: None,
            acceptance: Some("Prompt includes task guidance".into()),
            verify: None,
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        }),
        ..Default::default()
    })
    .await
    .expect("no-tools session should build with saved auth");

    let prompt = session
        .agent
        .as_ref()
        .expect("agent present")
        .system_prompt
        .clone();
    assert!(!prompt.trim().is_empty());
    assert!(prompt.contains("Test task"));
    assert!(prompt.contains("Verify headless prompt assembly"));
    assert!(prompt.contains("Available tools:"));
    assert!(!prompt.contains("- bash:"));
    assert!(!prompt.contains("- read:"));
    assert!(session
        .agent
        .as_ref()
        .expect("agent present")
        .tools
        .is_empty());
}

#[tokio::test]
async fn recv_event_returns_none_after_agent_end_even_if_sender_is_still_owned() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let (agent, handle) = Agent::new(
        clone_model(&test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            },
        })])),
        cwd.clone(),
    );

    let mut session = ImpSession {
        agent: Some(agent),
        handle,
        session_mgr: SessionManager::in_memory(),
        config: Config::default(),
        model: test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            },
        })]),
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    session.prompt("latest").await.unwrap();
    while let Some(event) = session.recv_event().await {
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            break;
        }
    }

    let next = tokio::time::timeout(std::time::Duration::from_secs(1), session.recv_event())
        .await
        .expect("recv_event should not hang after agent end");
    assert!(next.is_none());

    session.wait().await.unwrap();
}

#[tokio::test]
async fn abort_marks_wait_as_cancelled() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let (agent, handle) = Agent::new(
        test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            },
        })]),
        cwd.clone(),
    );
    let mut session = ImpSession {
        agent: Some(agent),
        handle,
        session_mgr: SessionManager::in_memory(),
        config: Config::default(),
        model: test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
            message: AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            },
        })]),
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: Some(tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            (
                Agent::new(
                    test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
                        message: AssistantMessage {
                            content: vec![ContentBlock::Text {
                                text: "done".into(),
                            }],
                            usage: None,
                            stop_reason: StopReason::EndTurn,
                            timestamp: 1,
                        },
                    })]),
                    PathBuf::from("/tmp"),
                )
                .0,
                Ok(()),
            )
        })),
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    session.abort();
    let result = session.wait().await;
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[tokio::test]
async fn prompt_uses_session_history_without_duplicate_active_prompt() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
        message: AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 42,
        },
    })]);
    let mut session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    session_mgr
        .append(SessionEntry::Message {
            id: "existing-user".into(),
            parent_id: None,
            message: imp_llm::Message::user("earlier"),
        })
        .unwrap();

    let (agent, handle) = Agent::new(clone_model(&model), cwd.clone());
    let mut session = ImpSession {
        agent: Some(agent),
        handle,
        session_mgr,
        config: Config::default(),
        model,
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    session.prompt("latest").await.unwrap();
    while let Some(event) = session.recv_event().await {
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            break;
        }
    }
    session.wait().await.unwrap();

    let messages: Vec<_> = session.session_mgr.get_active_messages();
    assert_eq!(messages.len(), 3);
    match &messages[0] {
        imp_llm::Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "earlier"),
            other => panic!("unexpected user content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[1] {
        imp_llm::Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "latest"),
            other => panic!("unexpected user content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[2] {
        imp_llm::Message::Assistant(assistant) => match assistant.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "done"),
            other => panic!("unexpected assistant content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
}

#[tokio::test]
async fn prompt_uses_compacted_active_history_for_follow_up_turns() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model_with_events(vec![Ok(StreamEvent::MessageEnd {
        message: AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "follow-up done".into(),
            }],
            usage: None,
            stop_reason: StopReason::EndTurn,
            timestamp: 99,
        },
    })]);
    let mut session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    session_mgr
        .append(SessionEntry::Message {
            id: "u1".into(),
            parent_id: None,
            message: imp_llm::Message::user("older request"),
        })
        .unwrap();
    session_mgr
        .append(SessionEntry::Message {
            id: "a1".into(),
            parent_id: None,
            message: imp_llm::Message::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "older answer".into(),
                }],
                usage: None,
                stop_reason: StopReason::EndTurn,
                timestamp: 1,
            }),
        })
        .unwrap();
    session_mgr
        .append(SessionEntry::Message {
            id: "u2".into(),
            parent_id: None,
            message: imp_llm::Message::user("recent request"),
        })
        .unwrap();
    session_mgr
        .append(SessionEntry::Compaction {
            id: "c1".into(),
            parent_id: None,
            summary: "[CONTEXT COMPACTION] compacted summary".into(),
            first_kept_id: "u2".into(),
            tokens_before: 100,
            tokens_after: 40,
        })
        .unwrap();

    let (agent, handle) = Agent::new(clone_model(&model), cwd.clone());
    let mut session = ImpSession {
        agent: Some(agent),
        handle,
        session_mgr,
        config: Config::default(),
        model,
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    session.prompt("new follow-up").await.unwrap();
    while let Some(event) = session.recv_event().await {
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            break;
        }
    }
    session.wait().await.unwrap();

    let messages = session.session_mgr.get_active_messages();
    assert_eq!(messages.len(), 4);
    match &messages[0] {
        imp_llm::Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert!(text.contains("CONTEXT COMPACTION")),
            other => panic!("unexpected summary content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[1] {
        imp_llm::Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "recent request"),
            other => panic!("unexpected recent user content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
    match &messages[2] {
        imp_llm::Message::User(user) => match user.content.as_slice() {
            [ContentBlock::Text { text }] => assert_eq!(text, "new follow-up"),
            other => panic!("unexpected follow-up content: {other:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn persist_event_entries_writes_assistant_and_canonical_usage() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model();
    let session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let (_agent, handle) = Agent::new(clone_model(&model), cwd.clone());

    let mut session = ImpSession {
        agent: None,
        handle,
        session_mgr,
        config: Config::default(),
        model,
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    let message = test_assistant_message(
        123,
        Some(Usage {
            input_tokens: 1_000,
            output_tokens: 250,
            cache_read_tokens: 100,
            cache_write_tokens: 50,
        }),
    );

    let persisted = session.persist_event_entries(&AgentEvent::TurnEnd {
        index: 2,
        message: message.clone(),
        workflow_review: crate::workflow_review::TurnWorkflowReview::no_change(2),
    });

    assert_eq!(persisted, vec!["assistant message", "canonical usage"]);

    let usage_records = session.session_mgr.usage_records();
    assert_eq!(usage_records.len(), 1);
    let record = &usage_records[0];
    assert_eq!(record.turn_index, Some(2));
    assert_eq!(record.provider.as_deref(), Some("test-provider"));
    assert_eq!(record.model.as_deref(), Some("test-model"));
    assert!(record.request_id.starts_with("assistant:"));
    assert!(record.assistant_message_id.is_some());
    let cost = record.cost.as_ref().unwrap();
    assert!((cost.input - 0.002).abs() < 1e-12);
    assert!((cost.output - 0.001).abs() < 1e-12);
    assert!((cost.cache_read - 0.00005).abs() < 1e-12);
    assert!((cost.cache_write - 0.00005).abs() < 1e-12);
    assert!((cost.total - 0.0031).abs() < 1e-12);
}

#[test]
fn persist_event_entries_skips_usage_record_when_usage_missing() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model();
    let session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let (_agent, handle) = Agent::new(clone_model(&model), cwd.clone());

    let mut session = ImpSession {
        agent: None,
        handle,
        session_mgr,
        config: Config::default(),
        model,
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    let persisted = session.persist_event_entries(&AgentEvent::TurnEnd {
        index: 0,
        message: test_assistant_message(456, None),
        workflow_review: crate::workflow_review::TurnWorkflowReview::no_change(0),
    });

    assert_eq!(persisted, vec!["assistant message"]);
    assert!(session.session_mgr.usage_records().is_empty());
}

#[test]
fn persist_event_entries_writes_sanitized_browser_events() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model();
    let mut session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let mut event = crate::agent::BrowserEvent::new(crate::agent::BrowserEventKind::Navigated);
    event.session_id = Some("browser-1".into());
    event.domain = Some("example.com".into());
    let persisted = session_mgr
        .persist_agent_event_entries(&model, &AgentEvent::Browser { event })
        .unwrap();
    assert_eq!(persisted, vec!["browser event"]);
    assert!(session_mgr.entries().iter().any(|entry| matches!(
        entry,
        SessionEntry::Custom { custom_type, data, .. }
            if custom_type == crate::session::BROWSER_EVENT_CUSTOM_TYPE
                && data["domain"] == "example.com"
                && data.get("value").is_none()
    )));
}

#[test]
fn persist_event_entries_writes_tool_results() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let session_dir = tmp.path().join("sessions");
    let model = test_model();
    let session_mgr = SessionManager::new(&cwd, &session_dir).unwrap();
    let (agent, handle) = Agent::new(clone_model(&model), cwd.clone());
    std::fs::create_dir_all(&cwd).unwrap();
    let file = cwd.join("tracked.rs");
    std::fs::write(&file, "original").unwrap();
    let checkpoint = agent
        .checkpoint_state
        .snapshot_paths(
            std::slice::from_ref(&file),
            Some("before tool result".into()),
        )
        .unwrap()
        .unwrap();
    std::fs::write(&file, "modified").unwrap();

    let mut session = ImpSession {
        agent: Some(agent),
        handle,
        session_mgr,
        config: Config::default(),
        model,
        auth_store: AuthStore::new(tmp.path().join("auth.json")),
        model_registry: ModelRegistry::with_builtins(),
        cwd,
        agent_task: None,
        completed_run_result: None,
        pending_persistence_errors: VecDeque::new(),
        context_prefill: Vec::new(),
        context_prefill_injected: false,
    };

    let persisted = session.persist_event_entries(&AgentEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        result: imp_llm::ToolResultMessage {
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            content: vec![ContentBlock::Text { text: "ok".into() }],
            is_error: false,
            details: json!({"exit_code": 0}),
            timestamp: 999,
        },
        provenance: None,
    });

    assert_eq!(persisted, vec!["tool result"]);
    assert!(session.session_mgr.entries().iter().any(|entry| matches!(
        entry,
        SessionEntry::Message {
            message: imp_llm::Message::ToolResult(_),
            ..
        }
    )));
    let checkpoints = session.session_mgr.checkpoint_records();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].checkpoint_id, checkpoint.id);
    let restored = session
        .session_mgr
        .restore_checkpoint(
            session
                .agent
                .as_ref()
                .expect("agent retained for persistence test")
                .checkpoint_state
                .as_ref(),
            &checkpoints[0].checkpoint_id,
        )
        .unwrap();
    assert_eq!(restored, vec![file.clone()]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
}
