use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use imp_core::agent::AgentCommand;
use imp_core::builder::AgentBuilder;
use imp_core::config::Config;
use imp_core::session::SessionManager;
use imp_core::tools::ToolRegistry;
use imp_core::{Error as ImpCoreError, WorkflowUnitRef};
use imp_llm::auth::AuthStore;
use imp_llm::model::ModelRegistry;
use imp_llm::providers::create_provider;
use imp_llm::{ContentBlock, Message, Model, ThinkingLevel};

use super::{
    agent_event_kind, resolve_provider_api_key, should_use_chatgpt_provider, trace_tui_to,
    RuntimeSignal, TuiTrace,
};

pub(super) struct AgentStartRequest {
    pub(super) session: SessionManager,
    pub(super) model_name: String,
    pub(super) model_registry: ModelRegistry,
    pub(super) role_name: Option<String>,
    pub(super) thinking_level: ThinkingLevel,
    pub(super) config: Config,
    pub(super) active_workflow_scope: Option<WorkflowUnitRef>,
    pub(super) runtime_signal_tx: tokio::sync::mpsc::UnboundedSender<RuntimeSignal>,
    pub(super) ui_tx: tokio::sync::mpsc::Sender<crate::tui_interface::UiRequest>,
    pub(super) preloaded_lua_tools: Option<ToolRegistry>,
    pub(super) prompt_context: Option<imp_core::builder::PromptContext>,
    pub(super) tui_trace: Option<TuiTrace>,
}

pub(super) struct AgentStartResult {
    pub(super) command_tx: tokio::sync::mpsc::Sender<AgentCommand>,
    pub(super) cancel_token: Arc<std::sync::atomic::AtomicBool>,
    pub(super) task: tokio::task::JoinHandle<Result<(), ImpCoreError>>,
    pub(super) event_task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for AgentStartResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentStartResult").finish_non_exhaustive()
    }
}
pub(super) fn start_agent_from_request(
    request: AgentStartRequest,
    prompt: &str,
    agent_cwd: PathBuf,
) -> Result<AgentStartResult, String> {
    let started = Instant::now();
    let trace = request.tui_trace.as_ref();

    let phase_started = Instant::now();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let mut meta = request
        .model_registry
        .resolve_meta(&request.model_name, None)
        .ok_or_else(|| format!("Unknown model: {}", request.model_name))?;
    let mut provider_name = meta.provider.clone();
    let role_registry = request
        .config
        .role_registry()
        .map_err(|err| format!("Role registry error: {err}"))?;
    let selected_role = request
        .role_name
        .as_deref()
        .map(|role_name| {
            role_registry
                .resolve(role_name)
                .ok_or_else(|| format!("Unknown role: {role_name}"))
        })
        .transpose()?;
    let cli_model_selected = request.config.model.as_deref() != Some(request.model_name.as_str());
    if !cli_model_selected {
        if let Some(role_name) = request.role_name.as_deref() {
            if let Some(model) =
                role_registry.resolve_model_for_role(role_name, &request.model_registry)
            {
                meta = model.clone();
                provider_name = meta.provider.clone();
            }
        }
    }
    if should_use_chatgpt_provider(&auth_store, &request.model_registry, &meta) {
        provider_name = "openai-codex".to_string();
        meta = request
            .model_registry
            .resolve_meta(&request.model_name, Some(&provider_name))
            .ok_or_else(|| format!("Unknown model: {}", request.model_name))?;
    }
    trace_tui_to(
        trace,
        format!(
            "agent_start_phase phase=model_provider duration_ms={}",
            phase_started.elapsed().as_millis()
        ),
    );

    let phase_started = Instant::now();
    let provider = create_provider(&provider_name)
        .ok_or_else(|| format!("Unknown provider: {provider_name}"))?;
    let api_key = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(resolve_provider_api_key(&mut auth_store, &provider_name))
    })
    .map_err(|e: imp_llm::Error| e.to_string())?;
    trace_tui_to(
        trace,
        format!(
            "agent_start_phase phase=auth duration_ms={}",
            phase_started.elapsed().as_millis()
        ),
    );
    let model = Model {
        meta,
        provider: Arc::from(provider),
    };

    let workflow_context = workflow_context_prompt_for_request(&request);
    let mut config = request.config.clone();
    config.thinking = Some(request.thinking_level);
    let requested_max_tokens = request.config.max_tokens;
    let builder_cwd_for_lua = agent_cwd.clone();
    let mut builder = AgentBuilder::new(config, agent_cwd, model, api_key);
    if let Some(role) = selected_role {
        builder = builder.role(role);
    }
    if let Some(prompt_context) = request.prompt_context.clone() {
        builder = builder.preloaded_prompt_context(prompt_context);
    }
    if let Some(preloaded_lua_tools) = request.preloaded_lua_tools {
        builder = builder.preloaded_lua_tools(preloaded_lua_tools);
    } else {
        let lua_cwd = builder_cwd_for_lua.clone();
        let user_config_dir = imp_core::config::Config::user_config_dir();
        builder = builder.lua_tool_loader(move |policy, tools| {
            imp_lua::init_lua_extensions(&user_config_dir, Some(&lua_cwd), tools, policy);
        });
    }
    let start = Instant::now();
    let (mut agent, handle) = builder
        .build()
        .map_err(|e: imp_core::error::Error| e.to_string())?;
    trace_tui_to(
        trace,
        format!(
            "agent_start_phase phase=builder_build duration_ms={}",
            start.elapsed().as_millis()
        ),
    );

    let tui_ui = crate::tui_interface::TuiInterface::new(request.ui_tx.clone());
    agent.ui = tui_ui;
    if let Some(max_tokens) = requested_max_tokens {
        agent.max_tokens = Some(max_tokens);
    }

    let phase_started = Instant::now();
    let mut messages: Vec<Message> = request.session.get_active_messages();
    if matches!(
        messages.last(),
        Some(Message::User(user))
            if matches!(
                user.content.as_slice(),
                [ContentBlock::Text { text }] if text == prompt
            )
    ) {
        messages.pop();
    }
    imp_core::session::sanitize_messages(&mut messages);
    agent.messages = messages;
    if let Some(workflow_context) = workflow_context {
        agent.messages.push(Message::user(workflow_context));
    }
    trace_tui_to(
        trace,
        format!(
            "agent_start_phase phase=session_messages duration_ms={} messages={}",
            phase_started.elapsed().as_millis(),
            agent.messages.len()
        ),
    );

    let phase_started = Instant::now();
    let prompt = prompt.to_string();
    let task = tokio::spawn(async move { agent.run(prompt).await });
    let command_tx = handle.command_tx.clone();
    let cancel_token = Arc::clone(&handle.cancel_token);
    let signal_tx = request.runtime_signal_tx.clone();
    let bridge_trace = request.tui_trace.clone();
    let mut event_rx = handle.event_rx;
    let event_task = tokio::spawn(async move {
        let bridge_started = Instant::now();
        while let Some(event) = event_rx.recv().await {
            trace_tui_to(
                bridge_trace.as_ref(),
                format!(
                    "agent_event_bridge received kind={} elapsed_ms={}",
                    agent_event_kind(&event),
                    bridge_started.elapsed().as_millis()
                ),
            );
            if signal_tx.send(RuntimeSignal::AgentEvent(event)).is_err() {
                break;
            }
            trace_tui_to(
                bridge_trace.as_ref(),
                format!(
                    "agent_event_bridge sent elapsed_ms={}",
                    bridge_started.elapsed().as_millis()
                ),
            );
        }
    });
    trace_tui_to(
        trace,
        format!(
            "agent_start_phase phase=spawn_tasks duration_ms={}",
            phase_started.elapsed().as_millis()
        ),
    );
    trace_tui_to(
        trace,
        format!(
            "agent_start_total duration_ms={}",
            started.elapsed().as_millis()
        ),
    );

    Ok(AgentStartResult {
        command_tx,
        cancel_token,
        task,
        event_task,
    })
}

pub(super) fn agent_start_join_result_to_signal(
    result: Result<Result<AgentStartResult, String>, tokio::task::JoinError>,
) -> RuntimeSignal {
    match result {
        Ok(Ok(result)) => RuntimeSignal::AgentStartCompleted(result),
        Ok(Err(error)) => RuntimeSignal::AgentStartFailed(error),
        Err(error) => RuntimeSignal::AgentStartFailed(format!("Agent start task failure: {error}")),
    }
}

fn workflow_context_prompt_for_request(request: &AgentStartRequest) -> Option<String> {
    let mut context = String::new();
    if let Some(scope) = request.active_workflow_scope.as_ref() {
        if !context.is_empty() {
            context.push(' ');
        }
        let title = scope.title.trim();
        if title.is_empty() {
            context.push_str(&format!(" Active workflow scope: {}.", scope.id));
        } else {
            context.push_str(&format!(
                " Active workflow scope: {} — {}.",
                scope.id, title
            ));
        }
    }
    if context.is_empty() {
        None
    } else {
        Some(context)
    }
}
