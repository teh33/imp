//! High-level session API for driving imp programmatically.
//!
//! `ImpSession` is the primary public interface for embedding imp in other
//! Rust programs, building custom UIs, or driving agents from orchestrators.
//! It wires together config, auth, model resolution, agent construction,
//! session persistence, and the event stream — eliminating the boilerplate
//! that each run mode (interactive, print, headless, RPC) otherwise
//! duplicates.
//!
//! # Example
//!
//! ```no_run
//! use imp_core::imp_session::{ImpSession, SessionOptions, SessionChoice};
//!
//! # async fn example() -> imp_core::Result<()> {
//! let mut session = ImpSession::create(SessionOptions {
//!     cwd: std::env::current_dir()?,
//!     ..Default::default()
//! }).await?;
//!
//! session.prompt("What files are in the current directory?").await?;
//!
//! while let Some(event) = session.recv_event().await {
//!     println!("{event:?}");
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use imp_llm::auth::{ApiKey, AuthStore};
use imp_llm::model::{ModelMeta, ModelRegistry};
use imp_llm::providers::create_provider;
use imp_llm::{Model, ThinkingLevel};

use crate::agent::{Agent, AgentCommand, AgentEvent, AgentHandle};
use crate::builder::AgentBuilder;
use crate::config::{AgentMode, Config};
use crate::error::{Error, Result};
use crate::policy::RunPolicy;
use crate::session::{SessionCheckpointRecord, SessionEntry, SessionManager};
use crate::storage;
use crate::system_prompt::{Fact, TaskContext};
use crate::ui::UserInterface;

// ── Options ─────────────────────────────────────────────────────

/// How to initialize the session file.
#[derive(Debug, Clone, Default)]
pub enum SessionChoice {
    /// Fresh session, persisted to disk.
    #[default]
    New,
    /// No persistence.
    InMemory,
    /// Continue the most recent session for the working directory.
    Continue,
    /// Open a specific session file.
    Open(PathBuf),
    /// Open a specific session file, or create it at that path.
    OpenOrCreate(PathBuf),
}

use crate::tools::LuaToolLoader;
use crate::workflow::{AutonomyMode, VerificationGate};

/// Configuration for creating an `ImpSession`.
///
/// All fields have sensible defaults — only `cwd` is typically required.
pub struct SessionOptions {
    /// Working directory. Tools resolve paths relative to this.
    pub cwd: PathBuf,

    /// Prebuilt model override for deterministic tests or embedded callers.
    /// When set, ImpSession skips runtime model/provider/auth resolution.
    pub model_override: Option<Model>,

    /// Model hint — alias ("sonnet") or full ID. Resolved against the
    /// model registry. Falls back to config, then "sonnet".
    pub model: Option<String>,

    /// Provider override. Usually auto-detected from the model.
    pub provider: Option<String>,

    /// Runtime API key override (not persisted).
    pub api_key: Option<String>,

    /// Role to apply to this session. Alias or role id from the role registry.
    pub role: Option<String>,

    /// Thinking level override.
    pub thinking: Option<ThinkingLevel>,

    /// Agent mode (full, worker, orchestrator, …).
    pub mode: Option<AgentMode>,

    /// Autonomy mode for workflow/runtime policy. Defaults to safe.
    pub autonomy_mode: Option<AutonomyMode>,

    /// Verification gates declared by CLI/config/user input.
    pub verification_gates: Vec<VerificationGate>,

    /// Maximum turns before the agent stops.
    pub max_turns: Option<u32>,

    /// Resume workflow controller state from `.imp/runs/<run_id>/workflow-controller.json`.
    pub resume_run_id: Option<String>,

    /// Max output tokens per response.
    pub max_tokens: Option<u32>,

    /// Replace the assembled system prompt entirely.
    pub system_prompt: Option<String>,

    /// Skip native tool registration.
    pub no_tools: bool,

    /// Optional canonical tool allowlist applied after registration.
    pub enabled_tools: Option<Vec<String>>,

    /// Session persistence strategy.
    pub session: SessionChoice,

    /// Task context for headless / unit mode.
    pub task: Option<TaskContext>,

    /// Task-specific facts to inject into the system prompt.
    pub facts: Vec<Fact>,

    /// Lua extension loader. Called after native tools are registered.
    /// The binary crate typically provides this; library callers can
    /// pass `None` to skip Lua extensions.
    pub lua_loader: Option<LuaToolLoader>,

    /// Per-run tool/write policy layered on top of AgentMode.
    pub run_policy: RunPolicy,

    /// Custom UI implementation. Defaults to `NullInterface`.
    pub ui: Option<Arc<dyn UserInterface>>,

    /// Path to auth.json. Defaults to `~/.config/imp/auth.json`.
    pub auth_path: Option<PathBuf>,

    /// Pre-assembled context messages injected before the first prompt.
    /// Built by `context_prefill::assemble_context()` at dispatch time.
    /// The agent starts with these files already in its cached prefix.
    pub context_prefill: Vec<imp_llm::Message>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            model_override: None,
            model: None,
            provider: None,
            api_key: None,
            role: None,
            thinking: None,
            mode: None,
            autonomy_mode: None,
            verification_gates: Vec::new(),
            max_turns: None,
            resume_run_id: None,
            max_tokens: None,
            system_prompt: None,
            no_tools: false,
            enabled_tools: None,
            session: SessionChoice::default(),
            task: None,
            facts: Vec::new(),
            lua_loader: None,
            run_policy: RunPolicy::default(),
            ui: None,
            auth_path: None,
            context_prefill: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConnectionIntent<'a> {
    pub model_hint: Option<&'a str>,
    pub config_model: Option<&'a str>,
    pub provider_override: Option<&'a str>,
    pub api_key_override_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRuntimeConnection {
    pub model_id: String,
    pub provider_name: String,
}

/// Resolve the model-first runtime connection (model id + provider route/surface)
/// shared by CLI and session startup.
pub fn resolve_runtime_connection(
    intent: RuntimeConnectionIntent<'_>,
    auth_store: &AuthStore,
    registry: &ModelRegistry,
) -> std::result::Result<ResolvedRuntimeConnection, String> {
    let model_hint = intent
        .model_hint
        .or(intent.config_model)
        .unwrap_or("sonnet");

    let meta = registry
        .resolve_meta(model_hint, intent.provider_override)
        .ok_or_else(|| format!("Unknown model: {model_hint}"))?;

    let provider_name = intent
        .provider_override
        .unwrap_or(&meta.provider)
        .to_string();

    if let Some(oauth_route) = auth_preferred_oauth_route(
        intent.provider_override,
        intent.api_key_override_present,
        auth_store,
        registry,
        &meta,
        &provider_name,
    ) {
        return Ok(oauth_route);
    }

    Ok(ResolvedRuntimeConnection {
        model_id: meta.id.clone(),
        provider_name,
    })
}

// ── ImpSession ──────────────────────────────────────────────────

/// A fully wired agent session.
///
/// Manages the lifecycle of a single agent: config resolution, model
/// selection, session persistence, and the event/command channels.
pub struct ImpSession {
    agent: Option<Agent>,
    handle: AgentHandle,
    session_mgr: SessionManager,
    config: Config,
    model: Model,
    auth_store: AuthStore,
    model_registry: ModelRegistry,
    cwd: PathBuf,
    /// Task handle for the currently running agent loop, if any.
    agent_task: Option<JoinHandle<(Agent, Result<()>)>>,
    completed_run_result: Option<Result<()>>,
    pending_persistence_errors: VecDeque<String>,
    pending_follow_ups: VecDeque<String>,
    /// Context prefill messages, injected once before the first prompt.
    context_prefill: Vec<imp_llm::Message>,
    context_prefill_injected: bool,
}

impl ImpSession {
    /// Create a new session by resolving config, auth, model, and tools.
    ///
    /// This is the main factory — mirrors pi's `createAgentSession()`.
    pub async fn create(options: SessionOptions) -> Result<Self> {
        let cwd = options.cwd.clone();

        let _ = storage::reconcile_legacy_into_global_root();

        // 1. Load config (user + project, merged)
        let mut config = Config::resolve(&Config::user_config_dir(), Some(&cwd))?;

        // Apply option overrides
        if let Some(thinking) = options.thinking {
            config.thinking = Some(thinking);
        }
        if let Some(mode) = options.mode {
            config.mode = mode;
        }

        // 2. Resolve auth
        let auth_path = options
            .auth_path
            .clone()
            .or_else(storage::existing_global_auth_path)
            .unwrap_or_else(storage::global_auth_path);
        let mut auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));

        if let Some(ref key) = options.api_key {
            // We'll set this after we know the provider name
            // Store it temporarily
            let _ = key; // handled below
        }

        // 3. Resolve model + provider route
        let model_registry = ModelRegistry::with_builtins();
        let role_registry = config
            .role_registry()
            .map_err(|err| Error::Config(err.to_string()))?;
        let selected_role = options
            .role
            .as_deref()
            .map(|role_name| {
                role_registry
                    .resolve(role_name)
                    .ok_or_else(|| Error::Config(format!("Unknown role: {role_name}")))
            })
            .transpose()?;
        let role_model_id = selected_role.as_ref().and_then(|role| {
            if options.model.is_some() {
                None
            } else {
                role_registry
                    .resolve_model_for_role(&role.name, &model_registry)
                    .map(|model| model.id.clone())
            }
        });
        let (model, _provider_name, api_key) = if let Some(model) = options.model_override.as_ref()
        {
            (
                clone_model(model),
                model.meta.provider.clone(),
                String::new(),
            )
        } else {
            let runtime_connection = resolve_runtime_connection(
                RuntimeConnectionIntent {
                    model_hint: options.model.as_deref().or(role_model_id.as_deref()),
                    config_model: config.model.as_deref(),
                    provider_override: options.provider.as_deref(),
                    api_key_override_present: options.api_key.is_some(),
                },
                &auth_store,
                &model_registry,
            )
            .map_err(Error::Config)?;

            let meta = model_registry
                .resolve_meta(
                    &runtime_connection.model_id,
                    Some(&runtime_connection.provider_name),
                )
                .ok_or_else(|| {
                    Error::Config(format!(
                        "Unknown model/provider route: {} via {}",
                        runtime_connection.model_id, runtime_connection.provider_name
                    ))
                })?;

            let provider_name = runtime_connection.provider_name.clone();

            if let Some(ref key) = options.api_key {
                auth_store.set_runtime_key(&provider_name, key.clone());
            }

            let provider = create_provider(&provider_name)
                .ok_or_else(|| Error::Config(format!("Unknown provider: {provider_name}")))?;

            let api_key = resolve_api_key(&mut auth_store, &provider_name).await?;
            (
                Model {
                    meta,
                    provider: Arc::from(provider),
                },
                provider_name,
                api_key,
            )
        };

        // 5. Build agent
        let mut builder =
            AgentBuilder::new(config.clone(), cwd.clone(), clone_model(&model), api_key);

        if let Some(role) = selected_role {
            builder = builder.role(role);
        }
        if let Some(task) = &options.task {
            builder = builder.task(task.clone());
        }
        if !options.facts.is_empty() {
            builder = builder.facts(options.facts.clone());
        }
        if let Some(prompt) = &options.system_prompt {
            builder = builder.system_prompt(prompt.clone());
        }
        builder = builder.no_tools(options.no_tools);
        if let Some(lua_loader) = options.lua_loader {
            builder = builder.lua_tool_loader(move |policy, tools| lua_loader(policy, tools));
        }
        if let Some(autonomy_mode) = options.autonomy_mode {
            builder = builder.autonomy_mode(autonomy_mode);
        }
        builder = builder.verification_gates(options.verification_gates.clone());
        builder = builder.run_policy(options.run_policy.clone());

        let (mut agent, handle) = builder.build()?;

        if let Some(enabled_tools) = &options.enabled_tools {
            agent
                .tools
                .retain(|name| enabled_tools.iter().any(|enabled| enabled == name));
            if !enabled_tools.iter().any(|enabled| enabled == "task") {
                agent
                    .task_state
                    .lock()
                    .expect("session task state lock")
                    .disable();
            }
        }

        if let Some(resume_run_id) = &options.resume_run_id {
            agent.resume_workflow_controller_from_project_run(resume_run_id)?;
        }

        if options.no_tools {
            agent.thinking_level = config.thinking.unwrap_or(ThinkingLevel::Off);
            if let Some(max_tokens) = options.max_tokens.or(config.max_tokens) {
                agent.max_tokens = Some(max_tokens);
            }
        } else if let Some(max_tokens) = options.max_tokens {
            agent.max_tokens = Some(max_tokens);
        }
        if let Some(ui) = &options.ui {
            agent.ui = Arc::clone(ui);
        }

        // 6. Set up session persistence
        let session_dir = storage::global_sessions_dir();
        let session_mgr = match options.session {
            SessionChoice::New => SessionManager::new(&cwd, &session_dir)?,
            SessionChoice::InMemory => SessionManager::in_memory(),
            SessionChoice::Continue => SessionManager::continue_recent(&cwd, &session_dir)?
                .unwrap_or_else(|| SessionManager::new(&cwd, &session_dir).unwrap()),
            SessionChoice::Open(ref path) => SessionManager::open(path)?,
            SessionChoice::OpenOrCreate(ref path) => SessionManager::open_or_create(&cwd, path)?,
        };

        let mut agent = agent;
        let stable_session_id = session_mgr.session_id();
        agent.session_id = stable_session_id.clone();
        agent.thread_id = stable_session_id;

        Ok(Self {
            agent: Some(agent),
            handle,
            session_mgr,
            config,
            model,
            auth_store,
            model_registry,
            cwd,
            context_prefill: options.context_prefill,
            context_prefill_injected: false,
            agent_task: None,
            completed_run_result: None,
            pending_persistence_errors: VecDeque::new(),
            pending_follow_ups: VecDeque::new(),
        })
    }

    // ── Prompting ───────────────────────────────────────────────

    /// Send a prompt and run the agent loop.
    ///
    /// The agent runs on a background task. Use [`recv_event`] to consume
    /// events, and [`steer`] / [`follow_up`] / [`cancel`] to control it.
    ///
    /// Returns an error if the agent is already running.
    pub async fn prompt(&mut self, text: &str) -> Result<()> {
        if self.agent_task.is_some() {
            return Err(Error::Config(
                "Agent is already running. Cancel or wait for it to finish.".into(),
            ));
        }

        self.completed_run_result = None;
        self.pending_persistence_errors.clear();
        self.pending_follow_ups.clear();

        // Persist user message to session
        let msg_id = uuid::Uuid::new_v4().to_string();
        let _ = self.session_mgr.append(SessionEntry::Message {
            id: msg_id,
            parent_id: None,
            message: imp_llm::Message::user(text),
        });

        // Load prior messages from session history into agent
        let mut agent = self
            .agent
            .take()
            .ok_or_else(|| Error::Config("Agent already consumed".into()))?;

        let mut history: Vec<imp_llm::Message> = self.session_mgr.get_active_messages();

        // The prompt was already appended to session history so resume/tree state
        // is correct, but Agent::run() will push the active prompt itself. Remove
        // the just-appended trailing user message to avoid duplicating it in the
        // model context for this run.
        if matches!(
            history.last(),
            Some(imp_llm::Message::User(user))
                if matches!(
                    user.content.as_slice(),
                    [imp_llm::ContentBlock::Text { text: last_text }] if last_text == text
                )
        ) {
            history.pop();
        }

        // Inject context prefill (once, before the first prompt). These messages
        // form the cached prefix: file contents the agent needs, assembled at
        // dispatch time by context_prefill::assemble_context(). Subsequent turns
        // get cache_read on this prefix instead of re-reading files.
        if !self.context_prefill_injected && !self.context_prefill.is_empty() {
            for msg in &self.context_prefill {
                history.push(msg.clone());
            }
            // Assistant acknowledgment to maintain user/assistant alternation
            history.push(imp_llm::Message::Assistant(imp_llm::AssistantMessage {
                content: vec![imp_llm::ContentBlock::Text {
                    text: "Context loaded. Ready to work.".into(),
                }],
                usage: None,
                stop_reason: imp_llm::StopReason::EndTurn,
                timestamp: imp_llm::now(),
            }));
            self.context_prefill_injected = true;
        }

        // Replace agent messages with session history. Agent::run() will append
        // the active prompt as the next user message.
        agent.messages = history;

        let prompt = text.to_string();
        let task = tokio::spawn(async move {
            let result = agent.run(prompt).await;
            (agent, result)
        });
        self.agent_task = Some(task);

        Ok(())
    }

    /// Send a prompt and block until the agent finishes.
    ///
    /// Events are still emitted via [`recv_event`], but this method
    /// does not return until the agent loop completes.
    pub async fn prompt_and_wait(&mut self, text: &str) -> Result<()> {
        self.prompt(text).await?;
        self.wait().await
    }

    /// Wait for the running agent to finish.
    pub async fn wait(&mut self) -> Result<()> {
        if let Some(task) = self.agent_task.take() {
            let (agent, result) = task
                .await
                .map_err(|e| Error::Config(format!("Agent task panicked: {e}")))?;
            self.agent = Some(agent);
            self.completed_run_result = Some(result);
            self.drain_pending_events_for_persistence();
        }

        if let Some(result) = self.completed_run_result.take() {
            return result;
        }

        Ok(())
    }

    /// Interrupt the agent: delivered after the current tool finishes,
    /// remaining queued tools are skipped.
    pub async fn steer(&mut self, text: &str) -> Result<()> {
        self.handle
            .command_tx
            .send(AgentCommand::Steer(text.into()))
            .await
            .map_err(|_| Error::Config("Agent not running".into()))?;
        self.session_mgr.append(SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            message: imp_llm::Message::user(text),
        })?;
        Ok(())
    }

    /// Follow-up: delivered only after the agent finishes all current work.
    pub async fn follow_up(&mut self, text: &str) -> Result<()> {
        self.handle
            .command_tx
            .send(AgentCommand::FollowUp(text.into()))
            .await
            .map_err(|_| Error::Config("Agent not running".into()))?;
        self.pending_follow_ups.push_back(text.to_string());
        Ok(())
    }

    /// Cancel the current agent run.
    pub async fn cancel(&self) -> Result<()> {
        self.handle
            .command_tx
            .send(AgentCommand::Cancel)
            .await
            .map_err(|_| Error::Config("Agent not running".into()))
    }

    /// Force-abort the current agent task when graceful cancellation does not finish.
    pub fn abort(&mut self) {
        if let Some(task) = self.agent_task.take() {
            task.abort();
            self.completed_run_result = Some(Err(Error::Cancelled));
        }
    }

    // ── Events ──────────────────────────────────────────────────

    /// Receive the next event from the agent.
    ///
    /// Returns `None` when the agent has finished and all events have
    /// been consumed.
    pub async fn recv_event(&mut self) -> Option<AgentEvent> {
        if let Some(error) = self.take_persistence_error() {
            return Some(AgentEvent::Error { error });
        }

        if self.agent_task.is_none() && self.completed_run_result.is_some() {
            return None;
        }

        let event = self.handle.event_rx.recv().await?;
        let events = self.persist_event_entries(&event);
        if matches!(event, AgentEvent::TurnEnd { .. }) {
            self.persist_next_follow_up();
        }

        if matches!(event, AgentEvent::AgentEnd { .. }) {
            if let Some(task) = self.agent_task.take() {
                match task.await {
                    Ok((agent, result)) => {
                        self.agent = Some(agent);
                        self.completed_run_result = Some(result);
                    }
                    Err(join_error) => {
                        self.push_persistence_error(
                            events,
                            format!("agent task panicked: {join_error}"),
                        );
                    }
                }
            }
        }

        Some(event)
    }

    fn persist_next_follow_up(&mut self) {
        let Some(text) = self.pending_follow_ups.pop_front() else {
            return;
        };
        if let Err(error) = self.session_mgr.append(SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            message: imp_llm::Message::user(&text),
        }) {
            self.pending_persistence_errors
                .push_back(format!("failed to persist follow-up message: {error}"));
        }
    }

    /// Get mutable access to the raw event receiver.
    ///
    /// Use this when you need `select!` or other channel combinators.
    pub fn event_rx(&mut self) -> &mut mpsc::UnboundedReceiver<AgentEvent> {
        &mut self.handle.event_rx
    }

    // ── Model ───────────────────────────────────────────────────

    /// Switch the model for subsequent prompts.
    ///
    /// The change takes effect on the next `prompt()` call.
    pub async fn set_model(&mut self, hint: &str) -> Result<()> {
        let meta = self
            .model_registry
            .resolve_meta(hint, None)
            .ok_or_else(|| Error::Config(format!("Unknown model: {hint}")))?;

        let provider_name = meta.provider.clone();
        let provider = create_provider(&provider_name)
            .ok_or_else(|| Error::Config(format!("Unknown provider: {provider_name}")))?;
        let api_key = resolve_api_key(&mut self.auth_store, &provider_name).await?;

        self.model = Model {
            meta,
            provider: Arc::from(provider),
        };

        // If we still have the agent (not currently running), update it
        if let Some(ref mut agent) = self.agent {
            agent.model = clone_model(&self.model);
            agent.api_key = api_key;
            let stable_session_id = self.session_mgr.session_id();
            agent.session_id = stable_session_id.clone();
            agent.thread_id = stable_session_id;
        }

        Ok(())
    }

    /// Set the thinking level for subsequent prompts.
    pub fn set_thinking(&mut self, level: ThinkingLevel) {
        self.config.thinking = Some(level);
        if let Some(ref mut agent) = self.agent {
            agent.thinking_level = level;
        }
    }

    // ── Accessors ───────────────────────────────────────────────

    /// The current model.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The resolved config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The session manager (tree, entries, persistence).
    pub fn session_manager(&self) -> &SessionManager {
        &self.session_mgr
    }

    /// Mutable access to the session manager.
    pub fn session_manager_mut(&mut self) -> &mut SessionManager {
        &mut self.session_mgr
    }

    /// The working directory.
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// The auth store (for checking credentials, OAuth status, etc).
    pub fn auth_store(&self) -> &AuthStore {
        &self.auth_store
    }

    /// Mutable access to the auth store.
    pub fn auth_store_mut(&mut self) -> &mut AuthStore {
        &mut self.auth_store
    }

    /// The model registry.
    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    /// Whether the agent is currently running a prompt.
    pub fn is_running(&self) -> bool {
        self.agent_task.is_some()
    }

    /// Get the raw command sender for advanced use cases.
    pub fn command_tx(&self) -> &mpsc::Sender<AgentCommand> {
        &self.handle.command_tx
    }

    fn persist_event_entries(&mut self, event: &AgentEvent) -> Vec<&'static str> {
        let persisted = match self
            .session_mgr
            .persist_agent_event_entries(&self.model, event)
        {
            Ok(persisted) => persisted,
            Err(error) => {
                self.push_persistence_error(
                    Vec::new(),
                    format!("failed to persist agent event entries: {error}"),
                );
                Vec::new()
            }
        };

        if let Some(agent) = self.agent.as_ref() {
            if let Err(error) =
                persist_checkpoint_records(&mut self.session_mgr, &agent.checkpoint_state)
            {
                self.push_persistence_error(
                    persisted.clone(),
                    format!("failed to persist checkpoint records: {error}"),
                );
            }
        }

        persisted
    }

    fn drain_pending_events_for_persistence(&mut self) {
        while let Ok(event) = self.handle.event_rx.try_recv() {
            self.persist_event_entries(&event);
        }
    }

    fn push_persistence_error(&mut self, persisted: Vec<&'static str>, error: String) {
        let prefix = if persisted.is_empty() {
            "session persistence warning".to_string()
        } else {
            format!("session persistence warning after {}", persisted.join(", "))
        };
        self.pending_persistence_errors
            .push_back(format!("{prefix}: {error}"));
    }

    fn take_persistence_error(&mut self) -> Option<String> {
        self.pending_persistence_errors.pop_front()
    }
}
// ── Helpers ─────────────────────────────────────────────────────

/// Resolve the API key for a provider, handling OAuth refresh.
async fn resolve_api_key(auth_store: &mut AuthStore, provider: &str) -> Result<ApiKey> {
    let result = match provider {
        "openai-codex" => auth_store.resolve_chatgpt_oauth().await,
        "anthropic" | "kimi-code" => auth_store.resolve_with_refresh(provider).await,
        _ => auth_store.resolve(provider),
    };
    result.map_err(|e| Error::Config(format!("Auth failed for {provider}: {e}")))
}

fn auth_preferred_oauth_route(
    provider_override: Option<&str>,
    api_key_override_present: bool,
    auth_store: &AuthStore,
    registry: &ModelRegistry,
    meta: &ModelMeta,
    provider_name: &str,
) -> Option<ResolvedRuntimeConnection> {
    if should_use_openai_chatgpt_route(
        provider_override,
        api_key_override_present,
        auth_store,
        registry,
        &meta.id,
        provider_name,
    ) {
        return Some(ResolvedRuntimeConnection {
            model_id: meta.id.clone(),
            provider_name: "openai-codex".to_string(),
        });
    }

    if should_use_kimi_code_route(
        provider_override,
        api_key_override_present,
        auth_store,
        registry,
        meta,
        provider_name,
    ) {
        return Some(ResolvedRuntimeConnection {
            model_id: "kimi2.6".to_string(),
            provider_name: "kimi-code".to_string(),
        });
    }

    None
}
fn should_use_openai_chatgpt_route(
    provider_override: Option<&str>,
    api_key_override_present: bool,
    auth_store: &AuthStore,
    registry: &ModelRegistry,
    model_id: &str,
    provider_name: &str,
) -> bool {
    let provider_allows_fallback = match provider_override {
        None => true,
        Some("openai") => true,
        Some(_) => false,
    };

    provider_allows_fallback
        && !api_key_override_present
        && provider_name == "openai"
        && auth_store.resolve_api_key_only("openai").is_err()
        && (auth_store.get_oauth("openai").is_some()
            || auth_store.get_oauth("openai-codex").is_some())
        && codex_supports_model(registry, model_id)
}

fn should_use_kimi_code_route(
    provider_override: Option<&str>,
    api_key_override_present: bool,
    auth_store: &AuthStore,
    registry: &ModelRegistry,
    meta: &ModelMeta,
    provider_name: &str,
) -> bool {
    let provider_allows_fallback = match provider_override {
        None => true,
        Some("moonshot") => true,
        Some("kimi-code") => true,
        Some(_) => false,
    };

    provider_allows_fallback
        && !api_key_override_present
        && provider_name == "moonshot"
        && auth_store.resolve_api_key_only("moonshot").is_err()
        && auth_store.get_oauth("kimi-code").is_some()
        && registry.find("kimi2.6").is_some()
        && is_kimi_moonshot_model(&meta.id)
}

fn is_kimi_moonshot_model(model_id: &str) -> bool {
    matches!(
        model_id,
        "kimi-k2.6"
            | "kimi-k2.5"
            | "kimi-k2-0905-preview"
            | "kimi-k2-turbo-preview"
            | "kimi-k2-thinking"
            | "kimi-k2-thinking-turbo"
    )
}
fn clone_model(model: &Model) -> Model {
    Model {
        meta: model.meta.clone(),
        provider: Arc::clone(&model.provider),
    }
}

fn persist_checkpoint_records(
    session_mgr: &mut SessionManager,
    checkpoint_state: &crate::tools::CheckpointState,
) -> Result<Vec<String>> {
    let existing: std::collections::HashSet<String> = session_mgr
        .checkpoint_records()
        .into_iter()
        .map(|record| record.checkpoint_id)
        .collect();

    let mut persisted = Vec::new();
    for record in checkpoint_state.checkpoints() {
        if existing.contains(&record.id) {
            continue;
        }
        session_mgr.append_checkpoint_record(SessionCheckpointRecord {
            version: crate::session::CHECKPOINT_RECORD_VERSION,
            checkpoint_id: record.id.clone(),
            created_at: record.created_at,
            label: record.label.clone(),
            files: record
                .files
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
        })?;
        persisted.push(record.id);
    }

    Ok(persisted)
}

fn codex_supports_model(_registry: &ModelRegistry, model_id: &str) -> bool {
    imp_llm::model::builtin_openai_codex_models()
        .iter()
        .any(|m| m.id == model_id)
}

#[cfg(test)]
mod tests;
