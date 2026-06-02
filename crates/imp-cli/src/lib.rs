use std::collections::{HashMap, VecDeque};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStage {
    ProcessStart,
    CwdResolved,
    ConfigResolved,
    SessionReady,
    AuthLoaded,
    ModelRegistryReady,
    ModelResolved,
    ProviderReady,
    ApiKeyResolved,
    AgentBuilt,
    PromptReady,
    RunLoopStarted,
}

impl StartupStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessStart => "process_start",
            Self::CwdResolved => "cwd_resolved",
            Self::ConfigResolved => "config_resolved",
            Self::SessionReady => "session_ready",
            Self::AuthLoaded => "auth_loaded",
            Self::ModelRegistryReady => "model_registry_ready",
            Self::ModelResolved => "model_resolved",
            Self::ProviderReady => "provider_ready",
            Self::ApiKeyResolved => "api_key_resolved",
            Self::AgentBuilt => "agent_built",
            Self::PromptReady => "prompt_ready",
            Self::RunLoopStarted => "run_loop_started",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupTiming {
    pub stage: StartupStage,
    pub since_start_ms: u64,
    pub since_previous_ms: u64,
}

#[derive(Debug)]
struct StartupTimer {
    started_at: std::time::Instant,
    last_mark_at: std::time::Instant,
    enabled: bool,
}

impl StartupTimer {
    fn new(enabled: bool) -> Self {
        let now = std::time::Instant::now();
        Self {
            started_at: now,
            last_mark_at: now,
            enabled,
        }
    }

    fn mark(&mut self, stage: StartupStage) -> Option<StartupTiming> {
        if !self.enabled {
            return None;
        }
        let now = std::time::Instant::now();
        let timing = StartupTiming {
            stage,
            since_start_ms: now.duration_since(self.started_at).as_millis() as u64,
            since_previous_ms: now.duration_since(self.last_mark_at).as_millis() as u64,
        };
        self.last_mark_at = now;
        Some(timing)
    }
}

use async_trait::async_trait;
use clap::{Args, Parser, Subcommand, ValueEnum};
use imp_core::agent::{Agent, AgentCommand, AgentEvent, AgentHandle};
use imp_core::config::{AgentMode, Config, ToolOutputDisplay};
use imp_core::format_error_for_display;
use imp_core::tools::web::types::SearchProvider;
use imp_core::tools::{
    AnchorStore, CheckpointState, FileCache, FileTracker, Tool, ToolContext, ToolOutput,
};
use imp_core::workflow::{AutonomyMode, VerificationGate};
use std::ffi::OsString;

use imp_core::imp_session::{
    resolve_runtime_connection, ImpSession, ResolvedRuntimeConnection, RuntimeConnectionIntent,
    SessionChoice, SessionOptions,
};
use imp_core::runtime::RuntimeStateAccumulator;
use imp_core::session::{SessionEntry, SessionManager};
use imp_core::ui::{ComponentSpec, NotifyLevel, SelectOption, UserInterface, WidgetContent};
use imp_core::usage::{UsageCostBreakdown, UsageRecordSource, UsageTokens};
use imp_core::TimingEvent;
use imp_llm::auth::{AuthStore, SecretFieldStatus, SecretStatus, StoredCredential};
use imp_llm::model::{ModelMeta, ModelRegistry, ProviderMeta, ProviderRegistry};
use imp_llm::oauth::anthropic::AnthropicOAuth;
use imp_llm::oauth::chatgpt::ChatGptOAuth;
use imp_llm::oauth::kimi_code::KimiCodeOAuth;
use imp_llm::provider::ThinkingLevel;
use imp_llm::providers::create_provider;
use imp_llm::{truncate_chars_with_suffix, Message, Model, StreamEvent};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

pub(crate) mod acp;
mod stats_report;
mod usage_report;

/// A coding agent engine
#[derive(Parser)]
#[command(name = "imp", version, about)]
struct Cli {
    /// Print response and exit (non-interactive mode)
    #[arg(short, long)]
    print: Option<String>,

    /// LLM provider (anthropic, openai, google)
    #[arg(long)]
    provider: Option<String>,

    /// Model to use (alias or full ID)
    #[arg(short, long)]
    model: Option<String>,
    /// Role to apply (planner, coder, verifier, reviewer, researcher, integrator)
    #[arg(long)]
    role: Option<String>,

    /// Thinking level: off, minimal, low, medium, high, xhigh
    #[arg(long)]
    thinking: Option<String>,

    /// API key override
    #[arg(long)]
    api_key: Option<String>,

    /// Continue most recent session
    #[arg(short, long)]
    #[clap(name = "continue")]
    cont: bool,

    /// Browse and select a session to resume
    #[arg(short, long)]
    resume: bool,

    /// Use a specific session file
    #[arg(long)]
    session: Option<PathBuf>,

    /// Ephemeral mode (no session persistence)
    #[arg(long)]
    no_session: bool,

    /// Enable specific tools (comma-separated)
    #[arg(long)]
    tools: Option<String>,

    /// Allow a tool by exact name for this run (repeatable)
    #[arg(long = "allow-tool")]
    allow_tools: Vec<String>,

    /// Deny a tool by exact name for this run (repeatable)
    #[arg(long = "deny-tool")]
    deny_tools: Vec<String>,

    /// Allow writes matching this path/glob relative to the worker cwd (repeatable)
    #[arg(long = "allow-write")]
    allow_writes: Vec<String>,

    /// Deny writes matching this path/glob relative to the worker cwd (repeatable)
    #[arg(long = "deny-write")]
    deny_writes: Vec<String>,

    /// Disable all built-in tools
    #[arg(long)]
    no_tools: bool,

    /// Replace default system prompt
    #[arg(long)]
    system_prompt: Option<String>,

    /// Output mode: interactive, rpc, json
    #[arg(long, default_value = "interactive")]
    mode: String,

    /// Final output format for --print: text or json
    #[arg(long, default_value = "text")]
    output: String,
    /// Emit shared runtime_event/runtime_state payloads alongside legacy JSON events
    #[arg(long)]
    runtime_json: bool,

    /// Autonomy mode: suggest, safe, local-auto, worktree-auto, allow-all-local, allow-all, ci
    #[arg(long, value_name = "MODE")]
    autonomy: Option<AutonomyMode>,

    /// Verification command gate to require for closeout. Repeat for multiple gates.
    #[arg(long = "verify", value_name = "COMMAND")]
    verify: Vec<String>,

    /// Maximum turns before stopping (default: 50)
    #[arg(long)]
    max_turns: Option<u32>,

    /// Max output tokens per response
    #[arg(long)]
    max_tokens: Option<u32>,

    /// Verbose startup logging
    #[arg(long)]
    verbose: bool,

    /// List available models
    #[arg(long)]
    list_models: bool,

    /// File arguments (@file includes file content)
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Open the terminal chat interface (legacy alias for `tui`)
    Chat,
    /// Run as an Agent Client Protocol stdio server
    Acp,
    /// Open the fullscreen terminal UI explicitly
    Tui,
    /// Manage Model Context Protocol server connections (planned)
    Mcp {
        #[command(subcommand)]
        command: Option<McpCommand>,
    },
    /// Open the viewer/inspector surface (planned; not fully implemented yet)
    View {
        /// Viewer area to open (planned: sessions, tree, logs, checkpoints)
        area: Option<String>,
    },
    /// Edit a guided subset of imp settings in the terminal
    Settings,
    /// Run the terminal-native setup wizard
    Setup,
    /// Log in to a provider. OAuth is supported for Anthropic, OpenAI/ChatGPT, and Kimi Code.
    Login {
        /// Provider to configure (`anthropic`, `openai`, `kimi`, or `kimi-code`). If omitted, prompts for a provider.
        provider: Option<String>,
    },
    /// Save, list, or remove API credentials in secure imp auth storage
    Secrets {
        #[command(subcommand)]
        command: Option<SecretsCommand>,
        /// Provider/service to configure (e.g. tavily, exa, resend, my-service)
        provider: Option<String>,
    },
    /// Edit configuration
    Config,
    /// Local statistics from persisted imp sessions
    Stats {
        #[command(subcommand)]
        command: StatsCommand,
    },
    /// Usage reporting and export
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    /// Inspect, validate, run, and update native workflow artifacts
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Open or inspect run evidence artifacts
    Evidence {
        #[command(subcommand)]
        command: Option<EvidenceCommand>,
    },
    /// Import skills and config from other agents (pi, Claude Code, Codex)
    Import {
        /// Only detect — don't copy anything
        #[arg(long)]
        dry_run: bool,
        /// Import from a specific agent: pi, claude, codex
        #[arg(long)]
        from: Option<String>,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Install this build to the user-visible `imp` command path
    InstallLocal {
        /// Print the chosen install destination without writing it
        #[arg(long)]
        dry_run: bool,
        /// Explicit install destination path
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    /// Save a web search provider API key into imp auth storage
    WebLogin {
        /// Search provider to configure (tavily, exa, linkup, perplexity)
        provider: String,
    },
}

#[derive(Subcommand, Debug)]
enum McpCommand {
    /// List configured MCP servers
    List,
    /// Add an MCP server to configuration
    Add,
    /// Remove an MCP server from configuration
    Remove,
    /// Diagnose MCP configuration and connectivity
    Doctor,
}

#[derive(Subcommand, Debug)]
enum WorkflowCommand {
    /// List workflows under .imp/workflows
    List,
    /// Show a workflow summary
    Show(WorkflowTargetArgs),
    /// Validate one workflow, or all workflows when no id is provided
    Validate(WorkflowValidateArgs),
    /// Show the next runnable workflow step
    Run(WorkflowTargetArgs),
    /// Update a workflow status path and append an audit event
    Update(WorkflowUpdateArgs),
}

#[derive(Args, Debug)]
struct WorkflowTargetArgs {
    /// Workflow id under .imp/workflows
    id: Option<String>,
}

#[derive(Args, Debug)]
struct WorkflowValidateArgs {
    /// Workflow id under .imp/workflows
    id: Option<String>,
    /// Validation mode
    #[arg(long, default_value = "strict")]
    mode: WorkflowValidationModeArg,
}

#[derive(Args, Debug)]
struct WorkflowUpdateArgs {
    /// Workflow id under .imp/workflows
    id: String,
    /// Workflow object path to replace, e.g. steps.verify.status
    path: String,
    /// Replacement status value
    value: String,
    /// Reason recorded in events.jsonl
    #[arg(long)]
    reason: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum WorkflowValidationModeArg {
    Draft,
    Strict,
}

impl WorkflowValidationModeArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Strict => "strict",
        }
    }
}

#[derive(Subcommand, Debug)]
enum EvidenceCommand {
    /// List recent run evidence records
    List,
    /// Print the latest evidence HTML path
    Latest,
}
#[derive(Subcommand, Debug)]
enum SecretsCommand {
    /// List configured secret providers/services
    List,
    /// Alias for list
    Ls,
    /// Show status for one configured provider/service
    Show {
        /// Provider/service to inspect
        provider: String,
    },
    /// Alias for show
    Inspect {
        /// Provider/service to inspect
        provider: String,
    },
    /// Verify that configured secrets are readable from secure storage
    Doctor,
    /// Remove a configured provider/service from secure storage
    Remove {
        /// Provider/service to remove
        provider: String,
    },
    /// Alias for remove
    Rm {
        /// Provider/service to remove
        provider: String,
    },
    /// Configure or update a provider/service's secret fields
    Set {
        /// Provider/service to configure (e.g. tavily, exa, resend, my-service)
        provider: String,
    },
}

#[derive(Subcommand, Debug)]
enum StatsCommand {
    /// Show overall local imp stats
    Summary(StatsReportArgs),
    /// Show token and cost stats
    Tokens(StatsReportArgs),
    /// Show stats grouped by tool
    Tools(StatsReportArgs),
    /// Show file/code-change stats
    Files(StatsReportArgs),
    /// Show stats grouped by day
    Daily(StatsReportArgs),
    /// Show stats grouped by week
    Weekly(StatsReportArgs),
    /// Show stats grouped by project/session directory hint
    Projects(StatsReportArgs),
    /// Show stats grouped by session
    Sessions(StatsReportArgs),
    /// Show a fun local imp wrapped summary
    Wrapped(StatsReportArgs),
    /// Export local imp stats records in a machine-friendly format
    Export(StatsExportArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum StatsExportFormat {
    Json,
}

#[derive(Debug, Clone, Args)]
struct StatsReportArgs {
    /// Include records on or after this unix timestamp or YYYY-MM-DD date
    #[arg(long)]
    since: Option<String>,
    /// Include records before this unix timestamp or date
    #[arg(long)]
    until: Option<String>,
    /// Only include this session id or path fragment
    #[arg(long)]
    session: Option<String>,
    /// Only include this tool name
    #[arg(long)]
    tool: Option<String>,
    /// Emit JSON instead of a human table when supported
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct StatsExportArgs {
    #[command(flatten)]
    filters: StatsReportArgs,
    /// Export format
    #[arg(long, value_enum, default_value_t = StatsExportFormat::Json)]
    format: StatsExportFormat,
}

#[derive(Debug, Clone)]
struct StatsFilters {
    since: Option<u64>,
    until: Option<u64>,
    session: Option<String>,
    tool: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StatsFilterSummary {
    since: Option<u64>,
    until: Option<u64>,
    session: Option<String>,
    tool: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct StatsSummaryJson {
    report: &'static str,
    generated_at: u64,
    filters: StatsFilterSummary,
    sessions: usize,
    tool_calls: usize,
    tool_errors: usize,
    unique_tools: usize,
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
    token_requests: usize,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
}

#[derive(Debug, Clone, Serialize)]
struct StatsToolRow {
    tool: String,
    calls: usize,
    errors: usize,
    files_created: usize,
    lines_added: usize,
    lines_removed: usize,
    lines_read: usize,
}

#[derive(Subcommand, Debug)]
enum UsageCommand {
    /// Show overall usage totals
    Summary(UsageReportArgs),
    /// Show usage grouped by day
    Daily(UsageReportArgs),
    /// Show usage grouped by model
    Models(UsageReportArgs),
    /// Show usage grouped by session
    Sessions(UsageReportArgs),
    /// Export usage records in a machine-friendly format
    Export(UsageExportArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
enum UsageExportFormat {
    Json,
}

#[derive(Debug, Clone, Args)]
struct UsageReportArgs {
    /// Include records on or after this unix timestamp or YYYY-MM-DD date
    #[arg(long)]
    since: Option<String>,
    /// Include records before this unix timestamp or date
    #[arg(long)]
    until: Option<String>,
    /// Only include this provider
    #[arg(long)]
    provider: Option<String>,
    /// Only include this model
    #[arg(long)]
    model: Option<String>,
    /// Only include this session id or path fragment
    #[arg(long)]
    session: Option<String>,
    /// Emit JSON instead of a human table when supported
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Args)]
struct UsageExportArgs {
    #[command(flatten)]
    filters: UsageReportArgs,
    /// Export format
    #[arg(long, value_enum, default_value_t = UsageExportFormat::Json)]
    format: UsageExportFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageReportKind {
    Summary,
    Daily,
    Models,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum UsageGroupKind {
    Day,
    Model,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundKind {
    Since,
    Until,
}

#[derive(Debug, Clone)]
struct UsageFilters {
    since: Option<u64>,
    until: Option<u64>,
    provider: Option<String>,
    model: Option<String>,
    session: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct UsageTotalsRow {
    requests: usize,
    tokens: UsageTokens,
    cost: UsageCostBreakdown,
}

#[derive(Debug, Clone, Serialize)]
struct UsageGroupRow {
    group: String,
    group_kind: UsageGroupKind,
    provider: Option<String>,
    model: Option<String>,
    session_id: Option<String>,
    session_path: Option<String>,
    day: Option<String>,
    totals: UsageTotalsRow,
}

#[derive(Debug, Clone, Serialize)]
struct UsageSessionSummary {
    session_id: Option<String>,
    session_path: Option<String>,
    messages: usize,
    first_timestamp: Option<u64>,
    last_timestamp: Option<u64>,
    first_day: Option<String>,
    last_day: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageFilterSummary {
    since: Option<u64>,
    until: Option<u64>,
    provider: Option<String>,
    model: Option<String>,
    session: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageSummaryJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    sessions: usize,
    providers: usize,
    models: usize,
    canonical_records: usize,
    legacy_records: usize,
}

#[derive(Debug, Clone, Serialize)]
struct UsageGroupedJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    rows: Vec<UsageGroupRow>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageExportJson {
    report: &'static str,
    generated_at: u64,
    filters: UsageFilterSummary,
    totals: UsageTotalsRow,
    records: Vec<UsageExportRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct UsageExportRecord {
    request_id: String,
    recorded_at: u64,
    day: String,
    provider: Option<String>,
    model: Option<String>,
    session: UsageSessionSummary,
    source: UsageRecordSource,
    tokens: UsageTokens,
    cost: Option<UsageCostBreakdown>,
    assistant_message_id: Option<String>,
    turn_index: Option<u32>,
    entry_id: String,
    parent_id: Option<String>,
}

fn split_path_entries(path: Option<OsString>) -> Vec<PathBuf> {
    path.as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect()
}

fn find_imp_on_path_from(path: Option<OsString>) -> Option<PathBuf> {
    split_path_entries(path)
        .into_iter()
        .map(|dir| dir.join("imp"))
        .find(|candidate| candidate.is_file())
}

fn path_contains_dir(path: Option<OsString>, dir: &std::path::Path) -> bool {
    split_path_entries(path)
        .into_iter()
        .any(|entry| entry == dir)
}

fn preferred_user_install_path(home: &std::path::Path, path: Option<OsString>) -> PathBuf {
    let home_bin = home.join("bin");
    if path_contains_dir(path.clone(), &home_bin) {
        return home_bin.join("imp");
    }

    let local_bin = home.join(".local/bin");
    if path_contains_dir(path.clone(), &local_bin) {
        return local_bin.join("imp");
    }

    home.join(".cargo/bin/imp")
}

fn resolve_install_destination(
    home: &std::path::Path,
    path: Option<OsString>,
    active_imp: Option<PathBuf>,
    dest_override: Option<PathBuf>,
) -> PathBuf {
    if let Some(dest) = dest_override {
        return dest;
    }

    if let Some(active) = active_imp {
        if active.starts_with(home) {
            return active;
        }
    }

    preferred_user_install_path(home, path)
}

fn install_binary_to(source: &std::path::Path, dest: &std::path::Path) -> io::Result<()> {
    let parent = dest.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Install destination has no parent: {}", dest.display()),
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let temp = dest.with_extension("tmp");
    std::fs::copy(source, &temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&temp, perms)?;
    }
    std::fs::rename(&temp, dest)?;
    Ok(())
}

fn run_install_local(
    dest_override: Option<PathBuf>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let path_env = std::env::var_os("PATH");
    let active_imp = find_imp_on_path_from(path_env.clone());
    let dest = resolve_install_destination(&home, path_env, active_imp.clone(), dest_override);

    if dry_run {
        println!("{}", dest.display());
        return Ok(());
    }

    install_binary_to(&current_exe, &dest)?;

    println!("Installed imp to {}", dest.display());
    if let Some(previous) = active_imp {
        if previous != dest {
            println!(
                "Updated active-user install target instead of Cargo bin shadow path. Previous `imp` path was {}.",
                previous.display()
            );
        }
    }

    let resolved_after = find_imp_on_path_from(std::env::var_os("PATH"));
    match resolved_after {
        Some(path) if path == dest => {
            println!("`imp` now resolves to {}", path.display());
        }
        Some(path) => {
            println!(
                "Installed to {}, but `imp` still resolves to {}. Adjust PATH or rerun with --dest {}.",
                dest.display(),
                path.display(),
                path.display()
            );
        }
        None => {
            println!(
                "Installed to {}, but `imp` is not currently on PATH. Add {} to PATH.",
                dest.display(),
                dest.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
        }
    }

    Ok(())
}

async fn run_workflow_command(command: &WorkflowCommand) -> imp_core::Result<()> {
    let (action, id, mode, path, value, reason) = match command {
        WorkflowCommand::List => ("list", None, None, None, None, None),
        WorkflowCommand::Show(args) => ("show", args.id.as_deref(), None, None, None, None),
        WorkflowCommand::Validate(args) => (
            "validate",
            args.id.as_deref(),
            Some(args.mode.as_str()),
            None,
            None,
            None,
        ),
        WorkflowCommand::Run(args) => ("run", args.id.as_deref(), None, None, None, None),
        WorkflowCommand::Update(args) => (
            "update",
            Some(args.id.as_str()),
            None,
            Some(args.path.as_str()),
            Some(args.value.as_str()),
            Some(args.reason.as_str()),
        ),
    };

    let mut params = serde_json::Map::from_iter([("action".to_string(), json!(action))]);
    if let Some(id) = id {
        params.insert("id".to_string(), json!(id));
    }
    if let Some(mode) = mode {
        params.insert("mode".to_string(), json!(mode));
    }
    if let Some(path) = path {
        params.insert("path".to_string(), json!(path));
    }
    if let Some(value) = value {
        params.insert("value".to_string(), json!(value));
    }
    if let Some(reason) = reason {
        params.insert("reason".to_string(), json!(reason));
    }

    let output = run_workflow_tool(Value::Object(params), AgentMode::Full).await?;
    print_tool_output(&output);
    if output.is_error {
        return Err(imp_core::error::Error::Tool(
            "workflow command failed".into(),
        ));
    }
    Ok(())
}

async fn run_workflow_tool(params: Value, mode: AgentMode) -> imp_core::Result<ToolOutput> {
    let (tx, _rx) = mpsc::channel(16);
    let (command_tx, _command_rx) = mpsc::channel(16);
    let cwd = std::env::current_dir()?;
    let tool = imp_core::tools::workflow::WorkflowTool;
    tool.execute(
        "cli-workflow",
        params,
        ToolContext {
            cwd,
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx: tx,
            command_tx,
            ui: std::sync::Arc::new(imp_core::ui::NullInterface),
            file_cache: std::sync::Arc::new(FileCache::new()),
            checkpoint_state: std::sync::Arc::new(CheckpointState::new()),
            file_tracker: std::sync::Arc::new(std::sync::Mutex::new(FileTracker::new())),
            anchor_store: std::sync::Arc::new(AnchorStore::new()),
            lua_tool_loader: None,
            mode,
            read_max_lines: 500,
            turn_workflow_review: std::sync::Arc::new(std::sync::Mutex::new(
                imp_core::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: std::sync::Arc::new(Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        },
    )
    .await
}

fn print_tool_output(output: &ToolOutput) {
    for block in &output.content {
        if let imp_llm::ContentBlock::Text { text } = block {
            println!("{text}");
        }
    }
}

fn run_mcp_command(command: Option<&McpCommand>) {
    match command {
        None => {
            println!(
                "imp mcp is planned, but MCP server management is not implemented in this build."
            );
            println!("Available placeholder commands: list, add, remove, doctor");
        }
        Some(McpCommand::List) => {
            println!("No MCP servers: MCP server management is not implemented in this build.");
        }
        Some(McpCommand::Add) => {
            eprintln!("imp mcp add is not implemented in this build.");
            std::process::exit(2);
        }
        Some(McpCommand::Remove) => {
            eprintln!("imp mcp remove is not implemented in this build.");
            std::process::exit(2);
        }
        Some(McpCommand::Doctor) => {
            println!("MCP support: not implemented in this build");
            println!("Expected future config locations:");
            println!("  {}", Config::user_config_dir().join("mcp.json").display());
            if let Ok(cwd) = std::env::current_dir() {
                println!("  {}", cwd.join(".imp/mcp.json").display());
            }
        }
    }
}

fn run_evidence_command(command: Option<&EvidenceCommand>) -> imp_core::Result<()> {
    let records =
        imp_core::run_evidence::read_index_records(imp_core::storage::global_run_index_path())?;
    match command.unwrap_or(&EvidenceCommand::List) {
        EvidenceCommand::List => {
            for record in records.iter().rev().take(20) {
                let status = record.status.as_deref().unwrap_or("running");
                println!(
                    "{}\t{}\t{}\t{}",
                    record.run_id,
                    status,
                    record.cwd.display(),
                    record.evidence_html_path.display()
                );
            }
        }
        EvidenceCommand::Latest => {
            if let Some(record) = records.last() {
                println!("{}", record.evidence_html_path.display());
            }
        }
    }
    Ok(())
}

pub async fn run() {
    let cli = Cli::parse();

    // Dispatch subcommands first
    if let Some(command) = &cli.command {
        match command {
            Commands::Chat => {
                if let Err(e) = run_interactive(&cli).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Acp => {
                if let Err(e) = acp::run_stdio_server(env!("CARGO_PKG_VERSION")).await {
                    eprintln!("ACP server failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Tui => {
                if let Err(e) = run_interactive(&cli).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Mcp { command } => {
                run_mcp_command(command.as_ref());
                return;
            }
            Commands::View { area } => {
                if let Err(e) = run_view_mode(&cli, area.as_deref()).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Settings => {
                if let Err(e) = run_settings_mode() {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Setup => {
                if let Err(e) = run_setup_mode().await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Login { provider } => {
                if let Err(e) = run_login_command(provider.as_deref()).await {
                    eprintln!("Login failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Secrets { command, provider } => {
                if let Err(e) = run_secrets_command(command.as_ref(), provider.as_deref()).await {
                    eprintln!("Secrets command failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Config => {
                let config_dir = Config::user_config_dir();
                let config_path = config_dir.join("config.toml");
                println!("{}", config_path.display());
                return;
            }
            Commands::Stats { command } => {
                if let Err(e) = stats_report::run_stats_command(command) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Usage { command } => {
                if let Err(e) = usage_report::run_usage_command(command) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Workflow { command } => {
                if let Err(e) = run_workflow_command(command).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Evidence { command } => {
                if let Err(e) = run_evidence_command(command.as_ref()) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Import { dry_run, from, yes } => {
                run_import(*dry_run, from.as_deref(), *yes);
                return;
            }
            Commands::InstallLocal { dry_run, dest } => {
                if let Err(e) = run_install_local(dest.clone(), *dry_run) {
                    eprintln!("Install failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::WebLogin { provider } => {
                if let Err(e) = run_web_login(provider).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
        }
    }

    // List models
    if cli.list_models {
        run_list_models();
        return;
    }

    if cli.mode == "chat" {
        eprintln!("Error: --mode chat has been removed. Use `imp` for the TUI or `imp -p \"...\"` for one-shot mode.");
        std::process::exit(2);
    }

    // Expand @file args into file content context
    let file_context = expand_file_args(&cli.args);

    // Read from stdin if piped
    let stdin_content = {
        if !std::io::stdin().is_terminal() {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).ok();
            if buf.is_empty() {
                None
            } else {
                Some(buf)
            }
        } else {
            None
        }
    };

    // Print mode
    if let Some(ref prompt) = cli.print {
        let full_prompt = build_full_prompt(prompt, &file_context, &stdin_content);
        if let Err(e) = run_print_mode(&cli, &full_prompt).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // If stdin was piped without -p, run in print mode with stdin as prompt
    if let Some(ref stdin) = stdin_content {
        let remaining = prompt_args(&cli.args);
        let instruction = remaining.join(" ");
        let full_prompt = build_full_prompt(&instruction, &file_context, &Some(stdin.clone()));
        if let Err(e) = run_print_mode(&cli, &full_prompt).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // If positional prompt text was provided without -p, treat it as a one-shot prompt.
    // @file arguments still contribute file context and are not included in the prompt text.
    let bare_prompt_args = prompt_args(&cli.args);
    if !bare_prompt_args.is_empty() {
        let full_prompt = build_full_prompt(&bare_prompt_args.join(" "), &file_context, &None);
        if let Err(e) = run_print_mode(&cli, &full_prompt).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Default interactive mode: fullscreen TUI
    if cli.mode == "interactive" {
        if let Err(e) = run_interactive(&cli).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // RPC / JSON modes (JSON-lines stdin/stdout protocol)
    match cli.mode.as_str() {
        "rpc" | "json" => {
            if let Err(e) = run_rpc_mode(&cli).await {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown mode: {other}. Use interactive, chat, rpc, or json.");
            std::process::exit(1);
        }
    }
}

fn format_price(price: f64) -> String {
    if price == 0.0 {
        "n/a".to_string()
    } else {
        format!("${price:.2}")
    }
}

fn run_list_models() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list();

    println!(
        "{:<40} {:<12} {:>8} {:>10} {:>10}",
        "MODEL", "PROVIDER", "CONTEXT", "$/M IN", "$/M OUT"
    );
    println!("{}", "-".repeat(84));

    for m in models {
        println!(
            "{:<40} {:<12} {:>7}k {:>10} {:>10}",
            m.id,
            m.provider,
            m.context_window / 1000,
            format_price(m.pricing.input_per_mtok),
            format_price(m.pricing.output_per_mtok),
        );
    }
}

fn canonical_provider_name(name: &str) -> String {
    provider_alias(name)
}

fn resolve_stored_provider_name(auth_store: &AuthStore, name: &str) -> Option<String> {
    let canonical = canonical_provider_name(name);
    if auth_store.stored.contains_key(&canonical) {
        return Some(canonical);
    }

    auth_store
        .stored
        .keys()
        .find(|stored| stored.eq_ignore_ascii_case(&canonical))
        .cloned()
}

fn oauth_login_success_message(auth_store: &AuthStore, provider: &str) -> String {
    auth_store
        .oauth_display_info(provider)
        .map(|info| info.login_message(provider))
        .unwrap_or_else(|| format!("Logged in to {provider} successfully."))
}

fn provider_alias(name: &str) -> String {
    match name.trim().to_lowercase().as_str() {
        "kimi" => "moonshot".to_string(),
        other => other.to_string(),
    }
}

fn kimi_api_login_success_message(auth_store: &AuthStore) -> String {
    let registry = ProviderRegistry::with_builtins();
    let provider = registry
        .find("moonshot")
        .expect("moonshot provider should exist");

    let auth_kind = match auth_store.stored.get("moonshot") {
        Some(StoredCredential::SecretFields { fields })
            if fields.len() == 1 && fields.first().map(String::as_str) == Some("api_key") =>
        {
            "API key"
        }
        Some(StoredCredential::ApiKey { .. }) => "API key",
        Some(StoredCredential::SecretFields { .. }) => "credentials",
        Some(StoredCredential::OAuth(_)) => "credentials",
        None => "credentials",
    };

    format!(
        "Configured {} in imp's secure auth store using a {}. You can now run `imp -m kimi` or `imp -m kimi-k2.6`.",
        provider.name, auth_kind
    )
}

fn search_provider_from_name(name: &str) -> Option<SearchProvider> {
    match name.trim().to_lowercase().as_str() {
        "tavily" => Some(SearchProvider::Tavily),
        "exa" => Some(SearchProvider::Exa),
        "linkup" => Some(SearchProvider::Linkup),
        "perplexity" => Some(SearchProvider::Perplexity),
        "github" => Some(SearchProvider::GitHub),
        _ => None,
    }
}

fn search_provider_docs_url(provider: SearchProvider) -> &'static str {
    match provider {
        SearchProvider::Tavily => "https://app.tavily.com/home",
        SearchProvider::Exa => "https://dashboard.exa.ai/api-keys",
        SearchProvider::Linkup => "https://app.linkup.so/api-keys",
        SearchProvider::Perplexity => "https://www.perplexity.ai/settings/api",
        SearchProvider::GitHub => "https://github.com/settings/tokens",
    }
}

fn parse_secret_field_names(input: &str) -> Vec<String> {
    let names: Vec<String> = input
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .collect();
    if names.is_empty() {
        vec!["api_key".to_string()]
    } else {
        names
    }
}

fn prompt_for_secret_fields(
    _provider_name: &str,
    display_name: &str,
    docs_hint: &str,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    if docs_hint.is_empty() {
        eprintln!("Saving credentials for {display_name}.");
    } else {
        eprintln!("Saving credentials for {display_name}. Get them at: {docs_hint}");
    }

    eprintln!("Field names (comma-separated) [api_key]:");
    eprint!("> ");
    io::stdout().flush().ok();
    let mut field_input = String::new();
    std::io::stdin().read_line(&mut field_input)?;
    let field_names = parse_secret_field_names(&field_input);

    let mut fields = HashMap::new();
    for field in field_names {
        eprintln!("Enter {field}:");
        eprint!("> ");
        io::stdout().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let value = input.trim().to_string();
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("No value entered for {field}. Aborting."),
            )
            .into());
        }
        fields.insert(field, value);
    }

    Ok(fields)
}

async fn run_web_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let provider = search_provider_from_name(provider_name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Unknown web provider: {provider_name}. Use one of: tavily, exa, linkup, perplexity"
            ),
        )
    })?;

    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let _env_key = provider.env_key_name();
    let fields = prompt_for_secret_fields(
        provider.name(),
        provider.name(),
        search_provider_docs_url(provider),
    )?;

    auth_store.store_secret_fields(provider.name(), fields)?;
    eprintln!(
        "Credentials saved for {} in secure imp auth storage (metadata: {}).",
        provider.name(),
        auth_path.display()
    );
    eprintln!(
        "The web tool will now auto-detect {} without requiring an exported env var.",
        provider.name()
    );

    Ok(())
}

async fn run_secrets_command(
    command: Option<&SecretsCommand>,
    provider: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Some(SecretsCommand::List) | Some(SecretsCommand::Ls) => run_secrets_list(),
        Some(SecretsCommand::Show { provider }) | Some(SecretsCommand::Inspect { provider }) => {
            run_secrets_show(provider)
        }
        Some(SecretsCommand::Remove { provider }) | Some(SecretsCommand::Rm { provider }) => {
            run_secrets_remove(provider)
        }
        Some(SecretsCommand::Doctor) => run_secrets_doctor(),
        Some(SecretsCommand::Set { provider }) => run_secrets_login(provider).await,
        None => {
            let provider = provider.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Usage: imp secrets <provider> | imp secrets list | imp secrets show <provider> | imp secrets rm <provider>",
                )
            })?;
            run_secrets_login(provider).await
        }
    }
}

#[derive(Debug)]
struct SecretListRow {
    id: String,
    display_name: String,
    kind: String,
    fields: String,
    status: String,
}

fn secret_status_label(status: Option<&SecretStatus>) -> String {
    let Some(status) = status else {
        return "unknown".to_string();
    };
    if status.is_usable() {
        return "ok".to_string();
    }

    let broken_fields: Vec<String> = status
        .fields
        .iter()
        .filter_map(|(field, field_status)| match field_status {
            SecretFieldStatus::Present => None,
            SecretFieldStatus::Missing => Some(format!("{field}:missing")),
            SecretFieldStatus::Error(_) => Some(format!("{field}:error")),
        })
        .collect();

    if broken_fields.is_empty() {
        "broken".to_string()
    } else {
        format!("broken ({})", broken_fields.join(", "))
    }
}

fn secret_kind_and_fields(entry: &StoredCredential) -> (String, String) {
    match entry {
        StoredCredential::OAuth(_) => ("oauth".to_string(), "access_token".to_string()),
        StoredCredential::ApiKey { .. } => ("api_key".to_string(), "api_key".to_string()),
        StoredCredential::SecretFields { fields } => {
            let kind = if fields.len() == 1 && fields.first().map(String::as_str) == Some("api_key")
            {
                "api_key".to_string()
            } else {
                format!("{} fields", fields.len())
            };
            (kind, fields.join(", "))
        }
    }
}

fn secret_status_detail(status: &SecretStatus) -> String {
    status
        .fields
        .iter()
        .map(|(field, field_status)| match field_status {
            SecretFieldStatus::Present => format!("{field}: ok"),
            SecretFieldStatus::Missing => format!("{field}: missing from secure storage"),
            SecretFieldStatus::Error(error) => format!("{field}: secure storage error: {error}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_secrets_list() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));

    if auth_store.stored.is_empty() {
        println!("No saved credentials.");
        return Ok(());
    }

    let registry = ProviderRegistry::with_builtins();
    let mut rows: Vec<SecretListRow> = auth_store
        .stored
        .iter()
        .map(|(name, entry)| {
            let display_name = registry
                .find(name)
                .map(|meta| meta.name.to_string())
                .unwrap_or_else(|| name.clone());
            let (kind, fields) = secret_kind_and_fields(entry);
            let status = secret_status_label(auth_store.secret_status(name).as_ref());
            SecretListRow {
                id: name.clone(),
                display_name,
                kind,
                fields,
                status,
            }
        })
        .collect();

    rows.sort_by(|a, b| a.id.cmp(&b.id));

    let provider_w = rows
        .iter()
        .map(|row| format!("{} ({})", row.display_name, row.id).len())
        .max()
        .unwrap_or(8)
        .max("Provider".len());
    let kind_w = rows
        .iter()
        .map(|row| row.kind.len())
        .max()
        .unwrap_or(4)
        .max("Kind".len());
    let status_w = rows
        .iter()
        .map(|row| row.status.len())
        .max()
        .unwrap_or(6)
        .max("Status".len());

    println!(
        "{:<provider_w$}  {:<kind_w$}  {:<status_w$}  Fields",
        "Provider",
        "Kind",
        "Status",
        provider_w = provider_w,
        kind_w = kind_w,
        status_w = status_w
    );
    println!(
        "{:-<provider_w$}  {:-<kind_w$}  {:-<status_w$}  {:-<6}",
        "",
        "",
        "",
        "",
        provider_w = provider_w,
        kind_w = kind_w,
        status_w = status_w
    );

    let has_broken = rows.iter().any(|row| row.status != "ok");
    for row in rows {
        println!(
            "{:<provider_w$}  {:<kind_w$}  {:<status_w$}  {}",
            format!("{} ({})", row.display_name, row.id),
            row.kind,
            row.status,
            row.fields,
            provider_w = provider_w,
            kind_w = kind_w,
            status_w = status_w
        );
    }

    if has_broken {
        eprintln!(
            "\nSome secret metadata points at missing secure-storage values. Re-save with `imp secrets <provider>` or run `imp secrets doctor` for details."
        );
    }

    Ok(())
}

fn run_secrets_show(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let requested_provider = canonical_provider_name(provider);
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let registry = ProviderRegistry::with_builtins();

    let provider =
        resolve_stored_provider_name(&auth_store, &requested_provider).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("No saved credentials for {requested_provider}."),
            )
        })?;
    let entry = auth_store.stored.get(&provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("No saved credentials for {provider}."),
        )
    })?;

    let display_name = registry
        .find(&provider)
        .map(|meta| meta.name.to_string())
        .unwrap_or_else(|| provider.to_string());

    let (kind, fields) = secret_kind_and_fields(entry);
    let status = auth_store.secret_status(&provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("No saved credentials for {provider}."),
        )
    })?;

    println!("Provider : {} ({})", display_name, provider);
    println!("Kind     : {}", kind);
    println!("Fields   : {}", fields);
    println!("Storage  : secure keychain + auth metadata");
    println!("Status   : {}", secret_status_label(Some(&status)));
    println!("Values   : hidden");
    println!("\n{}", secret_status_detail(&status));

    if !status.is_usable() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "Saved metadata for {provider} exists, but one or more secret values are missing or unreadable. Re-save with `imp secrets {provider}`."
            ),
        )
        .into());
    }

    Ok(())
}

fn run_secrets_doctor() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));

    if auth_store.stored.is_empty() {
        println!("No saved credentials.");
        return Ok(());
    }

    let registry = ProviderRegistry::with_builtins();
    let mut providers: Vec<_> = auth_store.stored.keys().cloned().collect();
    providers.sort();

    let mut broken = Vec::new();
    for provider in providers {
        let display_name = registry
            .find(&provider)
            .map(|meta| meta.name.to_string())
            .unwrap_or_else(|| provider.clone());
        let status = auth_store.secret_status(&provider).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("No saved credentials for {provider}."),
            )
        })?;
        println!(
            "{} ({}) — {}",
            display_name,
            provider,
            secret_status_label(Some(&status))
        );
        for line in secret_status_detail(&status).lines() {
            println!("  {line}");
        }
        if !status.is_usable() {
            broken.push(provider);
        }
    }

    if broken.is_empty() {
        return Ok(());
    }

    eprintln!(
        "\nBroken secrets: {}. Re-save each with `imp secrets <provider>`; metadata without keychain values cannot authenticate.",
        broken.join(", ")
    );
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} saved secret provider(s) are not usable", broken.len()),
    )
    .into())
}

fn run_secrets_remove(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let provider = provider_alias(provider);
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
    auth_store.remove(&provider)?;
    eprintln!("Removed saved credentials for {provider}.");
    Ok(())
}

async fn run_secrets_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let canonical_provider = provider_alias(provider_name);
    let registry = ProviderRegistry::with_builtins();
    let provider_meta = registry.find(&canonical_provider);
    let display_name = provider_meta.map(|p| p.name).unwrap_or(&canonical_provider);
    let docs_hint = provider_meta.map(|p| p.docs_url).unwrap_or("");

    let fields = prompt_for_secret_fields(&canonical_provider, display_name, docs_hint)?;
    auth_store.store_secret_fields(&canonical_provider, fields)?;
    eprintln!("Credentials saved for {display_name}.");
    Ok(())
}

/// Try to import existing Kimi CLI OAuth credentials from `~/.kimi/credentials/kimi-code.json`.
fn try_import_kimi_cli_credentials() -> Option<imp_llm::auth::OAuthCredential> {
    let path = std::path::PathBuf::from(std::env::var_os("HOME")?)
        .join(".kimi")
        .join("credentials")
        .join("kimi-code.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let access_token = json["access_token"].as_str()?.to_string();
    let refresh_token = json["refresh_token"].as_str()?.to_string();
    let expires_at = json["expires_at"].as_f64()? as u64;
    Some(imp_llm::auth::OAuthCredential {
        access_token,
        refresh_token,
        expires_at,
    })
}

async fn run_login_command(provider: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let provider_name = match provider {
        Some(provider) if !provider.trim().is_empty() => provider.trim().to_string(),
        _ => prompt_login_provider()?,
    };
    run_login(&provider_name).await
}

fn prompt_login_provider() -> Result<String, Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No provider specified. Use `imp login <provider>` with one of: anthropic, openai, kimi, kimi-code.",
        )
        .into());
    }

    let providers = [
        ("anthropic", "Anthropic / Claude"),
        ("openai", "OpenAI"),
        ("kimi", "Kimi Code"),
    ];
    println!("Choose an OAuth provider:");
    for (idx, (_id, label)) in providers.iter().enumerate() {
        println!("{}. {label}", idx + 1);
    }
    println!("Or type a provider id: anthropic, openai, kimi, kimi-code");

    let choice = prompt_input_line("Provider> ")?;
    let trimmed = choice.trim();
    if let Some((id, _label)) = providers
        .iter()
        .find(|(id, _)| trimmed.eq_ignore_ascii_case(id))
    {
        return Ok((*id).to_string());
    }
    if let Some((id, _label)) = trimmed
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|idx| providers.get(idx))
    {
        return Ok((*id).to_string());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Invalid provider selection. Use one of: anthropic, openai, kimi, kimi-code.",
    )
    .into())
}

async fn run_login(provider_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));

    let canonical_provider = provider_alias(provider_name);
    // For login, "kimi" maps to the Kimi Code OAuth flow rather than Moonshot API key.
    let login_provider = if provider_name.trim().eq_ignore_ascii_case("kimi") {
        "kimi-code"
    } else {
        &canonical_provider
    };

    if login_provider == "anthropic" {
        let oauth = AnthropicOAuth::new();

        eprintln!("Opening browser for Anthropic login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                || async {
                    eprintln!("Paste the authorization code or redirect URL:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).ok()?;
                    let trimmed = input.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                },
            )
            .await?;

        auth_store.store(
            "anthropic",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!("{}", oauth_login_success_message(&auth_store, "anthropic"));
    } else if login_provider == "openai" || login_provider == "openai-codex" {
        let oauth = ChatGptOAuth::new();

        eprintln!("Opening browser for OpenAI login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                || async {
                    eprintln!("Paste the authorization code or redirect URL:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).ok()?;
                    let trimmed = input.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                },
            )
            .await?;

        auth_store.store(
            "openai",
            imp_llm::auth::StoredCredential::OAuth(credential.clone()),
        )?;
        auth_store.store(
            "openai-codex",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!(
            "{}",
            oauth_login_success_message(&auth_store, "openai-codex")
        );
    } else if login_provider == "kimi-code" {
        // Try importing existing kimi-cli credentials first.
        if let Some(credential) = try_import_kimi_cli_credentials() {
            auth_store.store(
                "kimi-code",
                imp_llm::auth::StoredCredential::OAuth(credential),
            )?;
            eprintln!("Imported Kimi Code credentials from kimi-cli.");
            eprintln!("{}", oauth_login_success_message(&auth_store, "kimi-code"));
            return Ok(());
        }

        let oauth = KimiCodeOAuth::new();

        eprintln!("Opening browser for Kimi Code login...");
        eprintln!("If the browser doesn't open, visit the URL printed below.");

        let credential = oauth
            .login(
                |url| {
                    eprintln!("\n{url}\n");
                    let _ = open_url(url);
                },
                |msg| {
                    eprintln!("{msg}");
                },
            )
            .await?;

        auth_store.store(
            "kimi-code",
            imp_llm::auth::StoredCredential::OAuth(credential),
        )?;
        eprintln!("{}", oauth_login_success_message(&auth_store, "kimi-code"));
    } else if canonical_provider == "moonshot" {
        let registry = ProviderRegistry::with_builtins();
        let provider = registry
            .find("moonshot")
            .expect("moonshot provider should exist");

        eprintln!("Kimi uses a generated API key, not an OAuth browser flow in imp.");
        eprintln!(
            "Open {} to create a key from Moonshot / Kimi or Kimi Code, then paste it below.",
            provider.docs_url
        );
        let _ = open_url(&format!("https://{}", provider.docs_url));

        let value = prompt_input_line("api_key> ")?;
        if value.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "No api_key entered. Aborting.",
            )
            .into());
        }
        auth_store
            .store_secret_fields("moonshot", HashMap::from([("api_key".to_string(), value)]))?;
        eprintln!("{}", kimi_api_login_success_message(&auth_store));
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`imp login {provider_name}` is not supported. Use one of: anthropic, openai, kimi. For API-only providers, use `imp secrets {}`.",
                provider_alias(provider_name)
            ),
        )
        .into());
    }

    Ok(())
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()?;
    }
    Ok(())
}

fn provider_has_auth(auth_store: &AuthStore, meta: &ProviderMeta) -> bool {
    meta.env_vars.iter().any(|v| {
        std::env::var(v)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    }) || auth_store.has_credentials(meta.id)
        || (meta.id == "moonshot" && auth_store.has_credentials("kimi-code"))
}

fn save_auth_secret_fields(
    provider: &str,
    fields: HashMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
    auth_store.store_secret_fields(provider, fields)?;
    Ok(())
}

fn setup_visible_provider(provider_id: &str) -> bool {
    provider_id != "kimi-code"
}

fn setup_models_for_provider(registry: &ModelRegistry, provider_id: &str) -> Vec<ModelMeta> {
    let mut models: Vec<ModelMeta> = registry
        .list_by_provider(provider_id)
        .into_iter()
        .cloned()
        .collect();

    if provider_id == "openai" {
        for mut model in imp_llm::model::builtin_openai_codex_models() {
            if models.iter().any(|existing| existing.id == model.id) {
                continue;
            }
            model.provider = "openai".into();
            models.push(model);
        }
    }

    models
}

async fn run_setup_mode() -> Result<(), Box<dyn std::error::Error>> {
    let _ = imp_core::storage::reconcile_legacy_into_global_root();
    let cwd = std::env::current_dir()?;
    let mut config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;
    let auth_path = imp_core::storage::global_auth_path();
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let provider_registry = ProviderRegistry::with_builtins();
    let model_registry = ModelRegistry::with_builtins();

    println!("imp setup");
    println!("=========");

    let providers: Vec<&ProviderMeta> = provider_registry
        .list()
        .iter()
        .filter(|provider| setup_visible_provider(provider.id))
        .collect();
    println!("Providers:");
    for (idx, provider) in providers.iter().enumerate() {
        let status = if provider_has_auth(&auth_store, provider) {
            "configured"
        } else {
            "needs auth"
        };
        println!("{}. {} [{}]", idx + 1, provider.name, status);
    }
    let provider_choice = prompt_input_line("Select provider> ")?;
    let provider_index = provider_choice
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|idx| *idx < providers.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid provider selection"))?;
    let provider = &providers[provider_index];

    if !provider_has_auth(&auth_store, provider) {
        println!("No auth detected for {}.", provider.name);
        if provider.id == "anthropic" || provider.id == "openai" || provider.id == "moonshot" {
            let mode = prompt_input_line("Choose auth mode [login|key]> ")?;
            if mode.eq_ignore_ascii_case("login") {
                run_login(provider.id).await?;
            } else {
                println!("Enter api_key for {}", provider.name);
                let value = prompt_input_line("api_key> ")?;
                save_auth_secret_fields(
                    provider.id,
                    HashMap::from([("api_key".to_string(), value)]),
                )?;
            }
        } else {
            println!("Get a key at: {}", provider.docs_url);
            let value = prompt_input_line("api_key> ")?;
            save_auth_secret_fields(provider.id, HashMap::from([("api_key".to_string(), value)]))?;
        }
    }

    let models = setup_models_for_provider(&model_registry, provider.id);
    if models.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No built-in models found for provider {}", provider.id),
        )
        .into());
    }

    println!();
    println!("Models for {}:", provider.name);
    for (idx, model) in models.iter().enumerate().take(20) {
        println!("{}. {} ({})", idx + 1, model.name, model.id);
    }
    let model_choice = prompt_input_line("Select model> ")?;
    let model_index = model_choice
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|idx| *idx < models.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid model selection"))?;
    let model = &models[model_index];
    config.model = Some(model.id.clone());

    println!();
    println!("Thinking levels:");
    println!("1. off");
    println!("2. minimal");
    println!("3. low");
    println!("4. medium");
    println!("5. high");
    println!("6. xhigh");
    let thinking_choice = prompt_input_line("Select thinking> ")?;
    config.thinking = Some(match thinking_choice.trim() {
        "1" => ThinkingLevel::Off,
        "2" => ThinkingLevel::Minimal,
        "3" => ThinkingLevel::Low,
        "4" => ThinkingLevel::Medium,
        "5" => ThinkingLevel::High,
        "6" => ThinkingLevel::XHigh,
        _ => {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid thinking selection").into(),
            )
        }
    });

    println!();
    println!("Web search provider:");
    println!("1. none");
    println!("2. tavily");
    println!("3. exa");
    println!("4. linkup");
    println!("5. perplexity");
    let web_choice = prompt_input_line("Select web search provider> ")?;
    config.web.search_provider = match web_choice.trim() {
        "1" | "" => None,
        "2" => Some(SearchProvider::Tavily),
        "3" => Some(SearchProvider::Exa),
        "4" => Some(SearchProvider::Linkup),
        "5" => Some(SearchProvider::Perplexity),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid web provider selection",
            )
            .into())
        }
    };

    if let Some(web_provider) = config.web.search_provider {
        let web_provider_name = web_provider.name();
        let web_has_auth = auth_store
            .resolve_secret_field(web_provider_name, "api_key")
            .is_ok()
            || std::env::var(web_provider.env_key_name()).is_ok();
        if !web_has_auth {
            println!("No key detected for web provider {}.", web_provider_name);
            println!("Get a key at: {}", search_provider_docs_url(web_provider));
            let value = prompt_input_line("api_key> ")?;
            save_auth_secret_fields(
                web_provider_name,
                HashMap::from([("api_key".to_string(), value)]),
            )?;
        }
    }

    let saved_path = save_user_config(&config)?;
    println!();
    println!("Setup complete.");
    println!("Config saved to {}", saved_path.display());
    println!("Provider: {}", provider.name);
    println!("Model: {}", model.id);
    println!(
        "Thinking: {}",
        config.thinking.map(thinking_level_label).unwrap_or("off")
    );
    println!(
        "Web search: {}",
        web_search_provider_label(config.web.search_provider)
    );

    Ok(())
}

fn parse_thinking_level(s: &str) -> ThinkingLevel {
    match s.to_lowercase().as_str() {
        "off" => ThinkingLevel::Off,
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::XHigh,
        _ => ThinkingLevel::Off,
    }
}

fn resolve_model_and_provider(
    cli: &Cli,
    config: &Config,
    registry: &ModelRegistry,
    auth_store: &AuthStore,
) -> Result<(String, String), String> {
    let ResolvedRuntimeConnection {
        model_id,
        provider_name,
    } = resolve_runtime_connection(
        RuntimeConnectionIntent {
            model_hint: cli.model.as_deref(),
            config_model: config.model.as_deref(),
            provider_override: cli.provider.as_deref(),
            api_key_override_present: cli.api_key.is_some(),
        },
        auth_store,
        registry,
    )?;

    Ok((model_id, provider_name))
}

async fn resolve_provider_api_key(
    auth_store: &mut AuthStore,
    provider_name: &str,
) -> Result<imp_llm::auth::ApiKey, imp_llm::Error> {
    match provider_name {
        "openai-codex" => auth_store.resolve_chatgpt_oauth().await,
        "anthropic" | "kimi-code" => auth_store.resolve_with_refresh(provider_name).await,
        _ => auth_store.resolve(provider_name),
    }
}

fn build_lua_loader(no_tools: bool, cwd: PathBuf) -> Option<imp_core::tools::LuaToolLoader> {
    if no_tools {
        return None;
    }

    fn init_lua_tools(
        cwd: PathBuf,
        policy: &imp_core::config::LuaCapabilityPolicy,
        tools: &mut imp_core::tools::ToolRegistry,
    ) {
        let user_config_dir = Config::user_config_dir();
        imp_lua::init_lua_extensions(&user_config_dir, Some(&cwd), tools, policy);
    }

    Some(Arc::new(move |policy, tools| {
        init_lua_tools(cwd.clone(), policy, tools)
    }))
}

fn emit_startup_timing(timer: &mut StartupTimer, stage: StartupStage) {
    if let Some(timing) = timer.mark(stage) {
        eprintln!(
            "[startup stage={} total={}ms delta={}ms]",
            timing.stage.as_str(),
            timing.since_start_ms,
            timing.since_previous_ms,
        );
    }
}

fn format_timing_event(timing: &TimingEvent) -> String {
    let llm = timing
        .since_llm_request_start_ms
        .map(|ms| format!(" llm={ms}ms"))
        .unwrap_or_default();
    let duration = timing
        .duration_ms
        .map(|ms| format!(" duration={ms}ms"))
        .unwrap_or_default();
    let label = timing
        .label
        .as_ref()
        .map(|label| format!(" label={label}"))
        .unwrap_or_default();
    let success = timing
        .success
        .map(|success| format!(" success={success}"))
        .unwrap_or_default();

    format!(
        "[timing turn={} stage={} turn={}ms{}{}{}{}]",
        timing.turn,
        timing.stage.as_str(),
        timing.since_turn_start_ms,
        llm,
        duration,
        label,
        success,
    )
}

fn cli_verification_gates(commands: &[String]) -> Vec<VerificationGate> {
    commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            VerificationGate::command(format!("cli-verify-{}", index + 1), command.clone())
        })
        .collect()
}

#[derive(Debug)]
enum RpcInputCommand {
    Prompt(String),
    Cancel,
    Steer(String),
    FollowUp(String),
}

type UiResponseMap = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;
type RpcAgentJoinHandle = JoinHandle<(Agent, imp_core::Result<()>)>;

struct RpcUi {
    stdout_tx: mpsc::Sender<Value>,
    pending: UiResponseMap,
    next_request_id: Arc<AtomicU64>,
}

impl RpcUi {
    fn new(stdout_tx: mpsc::Sender<Value>) -> Self {
        Self {
            stdout_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn pending(&self) -> UiResponseMap {
        self.pending.clone()
    }

    async fn emit(&self, value: Value) {
        let _ = self.stdout_tx.send(value).await;
    }

    async fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = format!("q{}", self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (response_tx, response_rx) = oneshot::channel();

        self.pending.lock().await.insert(id.clone(), response_tx);
        self.emit(json!({
            "type": "ui_request",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await;

        match tokio::time::timeout(Duration::from_secs(60), response_rx).await {
            Ok(Ok(result)) => Some(result),
            Ok(Err(_)) | Err(_) => {
                self.pending.lock().await.remove(&id);
                None
            }
        }
    }
}

#[async_trait]
impl UserInterface for RpcUi {
    fn has_ui(&self) -> bool {
        true
    }

    async fn notify(&self, message: &str, level: NotifyLevel) {
        self.emit(json!({
            "type": "ui_request",
            "method": "notify",
            "params": {
                "message": message,
                "level": serde_json::to_value(level).unwrap_or(Value::Null),
            }
        }))
        .await;
    }

    async fn confirm(&self, title: &str, message: &str) -> Option<bool> {
        self.request(
            "confirm",
            json!({
                "title": title,
                "message": message,
            }),
        )
        .await?
        .as_bool()
    }

    async fn select_with_context(
        &self,
        title: &str,
        context: &str,
        options: &[SelectOption],
    ) -> Option<usize> {
        let result = self
            .request(
                "select",
                json!({
                    "title": title,
                    "context": context,
                    "options": serde_json::to_value(options).unwrap_or_else(|_| json!([])),
                }),
            )
            .await?;

        result.as_u64().map(|index| index as usize)
    }

    async fn input_with_context(
        &self,
        title: &str,
        context: &str,
        placeholder: &str,
    ) -> Option<String> {
        self.request(
            "input",
            json!({
                "title": title,
                "context": context,
                "placeholder": placeholder,
            }),
        )
        .await?
        .as_str()
        .map(ToOwned::to_owned)
    }

    async fn set_status(&self, key: &str, text: Option<&str>) {
        self.emit(json!({
            "type": "ui_request",
            "method": "set_status",
            "params": {
                "key": key,
                "text": text,
            }
        }))
        .await;
    }

    async fn set_widget(&self, key: &str, content: Option<WidgetContent>) {
        self.emit(json!({
            "type": "ui_request",
            "method": "set_widget",
            "params": {
                "key": key,
                "content": serde_json::to_value(content).unwrap_or(Value::Null),
            }
        }))
        .await;
    }

    async fn custom(&self, component: ComponentSpec) -> Option<Value> {
        self.request(
            "custom",
            json!({
                "component": serde_json::to_value(component).unwrap_or(Value::Null),
            }),
        )
        .await
    }
}

async fn run_rpc_mode(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let mut startup_timer = StartupTimer::new(cli.verbose);
    emit_startup_timing(&mut startup_timer, StartupStage::ProcessStart);
    let cwd = std::env::current_dir()?;
    emit_startup_timing(&mut startup_timer, StartupStage::CwdResolved);
    let config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;
    emit_startup_timing(&mut startup_timer, StartupStage::ConfigResolved);
    let registry = ModelRegistry::with_builtins();
    emit_startup_timing(&mut startup_timer, StartupStage::ModelRegistryReady);

    let stdout_tx = spawn_json_lines_stdout_writer();
    let rpc_ui = Arc::new(RpcUi::new(stdout_tx.clone()));

    let (command_tx, mut command_rx) = mpsc::channel(64);
    tokio::spawn(read_rpc_stdin(
        command_tx,
        rpc_ui.pending(),
        stdout_tx.clone(),
    ));

    let mut history: Vec<Message> = Vec::new();
    let mut queued_followups: VecDeque<String> = VecDeque::new();
    let mut active_command_tx: Option<mpsc::Sender<AgentCommand>> = None;
    let mut active_join: Option<RpcAgentJoinHandle> = None;
    let mut stdin_closed = false;

    loop {
        if let Some(join_handle) = active_join.as_mut() {
            tokio::select! {
                maybe_command = command_rx.recv() => {
                    match maybe_command {
                        Some(command) => {
                            process_rpc_command(
                                command,
                                cli,
                                &cwd,
                                &config,
                                &registry,
                                &stdout_tx,
                                &rpc_ui,
                                &history,
                                &mut queued_followups,
                                &mut active_command_tx,
                                &mut active_join,
                            ).await?;
                        }
                        None => stdin_closed = true,
                    }
                }
                join_result = join_handle => {
                    active_join = None;
                    active_command_tx = None;

                    match join_result {
                        Ok((agent, _result)) => {
                            history = agent.messages;
                        }
                        Err(error) => {
                            emit_protocol_error(&stdout_tx, format!("agent task failed: {error}")).await;
                        }
                    }

                    if let Some(prompt) = queued_followups.pop_front() {
                        let (command_tx, join_handle) = spawn_rpc_agent(
                            cli,
                            &cwd,
                            &config,
                            &registry,
                            history.clone(),
                            rpc_ui.clone(),
                            stdout_tx.clone(),
                            prompt,
                        )?;
                        active_command_tx = Some(command_tx);
                        active_join = Some(join_handle);
                    } else if stdin_closed {
                        break;
                    }
                }
            }
        } else {
            match command_rx.recv().await {
                Some(command) => {
                    process_rpc_command(
                        command,
                        cli,
                        &cwd,
                        &config,
                        &registry,
                        &stdout_tx,
                        &rpc_ui,
                        &history,
                        &mut queued_followups,
                        &mut active_command_tx,
                        &mut active_join,
                    )
                    .await?;
                }
                None => break,
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_rpc_command(
    command: RpcInputCommand,
    cli: &Cli,
    cwd: &Path,
    config: &Config,
    registry: &ModelRegistry,
    stdout_tx: &mpsc::Sender<Value>,
    rpc_ui: &Arc<RpcUi>,
    history: &[Message],
    queued_followups: &mut VecDeque<String>,
    active_command_tx: &mut Option<mpsc::Sender<AgentCommand>>,
    active_join: &mut Option<RpcAgentJoinHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        RpcInputCommand::Prompt(content) => {
            if active_join.is_some() {
                queued_followups.push_back(content);
            } else {
                let (command_tx, join_handle) = spawn_rpc_agent(
                    cli,
                    cwd,
                    config,
                    registry,
                    history.to_vec(),
                    rpc_ui.clone(),
                    stdout_tx.clone(),
                    content,
                )?;
                *active_command_tx = Some(command_tx);
                *active_join = Some(join_handle);
            }
        }
        RpcInputCommand::Cancel => {
            if let Some(command_tx) = active_command_tx.as_ref() {
                let _ = command_tx.send(AgentCommand::Cancel).await;
            }
        }
        RpcInputCommand::Steer(content) => {
            if let Some(command_tx) = active_command_tx.as_ref() {
                let _ = command_tx.send(AgentCommand::Steer(content)).await;
            } else {
                emit_protocol_error(stdout_tx, "cannot steer without an active agent").await;
            }
        }
        RpcInputCommand::FollowUp(content) => {
            if active_join.is_some() {
                queued_followups.push_back(content);
            } else {
                let (command_tx, join_handle) = spawn_rpc_agent(
                    cli,
                    cwd,
                    config,
                    registry,
                    history.to_vec(),
                    rpc_ui.clone(),
                    stdout_tx.clone(),
                    content,
                )?;
                *active_command_tx = Some(command_tx);
                *active_join = Some(join_handle);
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn spawn_rpc_agent(
    cli: &Cli,
    cwd: &Path,
    config: &Config,
    registry: &ModelRegistry,
    history: Vec<Message>,
    rpc_ui: Arc<RpcUi>,
    stdout_tx: mpsc::Sender<Value>,
    prompt: String,
) -> Result<(mpsc::Sender<AgentCommand>, RpcAgentJoinHandle), Box<dyn std::error::Error>> {
    let mut startup_timer = StartupTimer::new(cli.verbose);
    emit_startup_timing(&mut startup_timer, StartupStage::ProcessStart);
    let (mut agent, handle) = create_rpc_agent(cli, cwd, config, registry, history, rpc_ui)?;
    let command_tx = handle.command_tx.clone();

    tokio::spawn(forward_rpc_events(handle, stdout_tx));

    emit_startup_timing(&mut startup_timer, StartupStage::PromptReady);
    let join_handle = tokio::spawn(async move {
        let result = agent.run(prompt).await;
        (agent, result)
    });
    emit_startup_timing(&mut startup_timer, StartupStage::RunLoopStarted);

    Ok((command_tx, join_handle))
}

fn create_rpc_agent(
    cli: &Cli,
    cwd: &Path,
    config: &Config,
    registry: &ModelRegistry,
    history: Vec<Message>,
    rpc_ui: Arc<RpcUi>,
) -> Result<(Agent, AgentHandle), Box<dyn std::error::Error>> {
    let mut startup_timer = StartupTimer::new(cli.verbose);
    emit_startup_timing(&mut startup_timer, StartupStage::ProcessStart);
    let auth_path = imp_core::storage::global_auth_path();
    let mut auth_store =
        AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
    emit_startup_timing(&mut startup_timer, StartupStage::AuthLoaded);
    let (model_id, provider_name) =
        resolve_model_and_provider(cli, config, registry, &auth_store).map_err(io::Error::other)?;
    emit_startup_timing(&mut startup_timer, StartupStage::ModelResolved);

    let provider = create_provider(&provider_name)
        .ok_or_else(|| io::Error::other(format!("Unknown provider: {provider_name}")))?;
    emit_startup_timing(&mut startup_timer, StartupStage::ProviderReady);

    let meta = registry
        .resolve_meta(&model_id, Some(&provider_name))
        .ok_or_else(|| io::Error::other(format!("Model not found: {model_id}")))?;

    if let Some(ref key) = cli.api_key {
        auth_store.set_runtime_key(&provider_name, key.clone());
    }

    let api_key = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(resolve_provider_api_key(&mut auth_store, &provider_name))
    })?;
    emit_startup_timing(&mut startup_timer, StartupStage::ApiKeyResolved);
    let model = Model {
        meta,
        provider: Arc::from(provider),
    };

    // Apply CLI thinking level override to config.
    let mut agent_config = config.clone();
    if let Some(ref thinking) = cli.thinking {
        agent_config.thinking = Some(parse_thinking_level(thinking));
    }

    let rpc_ui_clone = rpc_ui.clone() as Arc<dyn UserInterface>;
    let lua_cwd = cwd.to_path_buf();
    let mut builder =
        imp_core::builder::AgentBuilder::new(agent_config, cwd.to_path_buf(), model, api_key)
            .lua_tool_loader(move |policy, tools| {
                let user_config_dir = Config::user_config_dir();
                imp_lua::init_lua_extensions(&user_config_dir, Some(&lua_cwd), tools, policy);
            });
    if let Some(ref prompt) = cli.system_prompt {
        builder = builder.system_prompt(prompt.clone());
    }
    let (mut agent, handle) = builder.build()?;
    emit_startup_timing(&mut startup_timer, StartupStage::AgentBuilt);
    agent.ui = rpc_ui_clone;
    agent.messages = history;

    Ok((agent, handle))
}

fn spawn_json_lines_stdout_writer() -> mpsc::Sender<Value> {
    let (stdout_tx, mut stdout_rx) = mpsc::channel::<Value>(256);

    tokio::spawn(async move {
        let mut stdout = BufWriter::new(tokio::io::stdout());
        while let Some(value) = stdout_rx.recv().await {
            let Ok(line) = serde_json::to_string(&value) else {
                continue;
            };

            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdout.write_all(b"\n").await.is_err() {
                break;
            }
            if stdout.flush().await.is_err() {
                break;
            }
        }
    });

    stdout_tx
}

async fn read_rpc_stdin(
    command_tx: mpsc::Sender<RpcInputCommand>,
    pending_ui: UiResponseMap,
    stdout_tx: mpsc::Sender<Value>,
) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Value>(trimmed) {
                    Ok(value) => {
                        if value.get("type").and_then(Value::as_str) == Some("ui_response") {
                            if let Err(error) = deliver_ui_response(value, &pending_ui).await {
                                emit_protocol_error(&stdout_tx, error).await;
                            }
                            continue;
                        }

                        match parse_rpc_command(&value) {
                            Ok(command) => {
                                if command_tx.send(command).await.is_err() {
                                    break;
                                }
                            }
                            Err(error) => emit_protocol_error(&stdout_tx, error).await,
                        }
                    }
                    Err(error) => {
                        emit_protocol_error(&stdout_tx, format!("invalid JSON input: {error}"))
                            .await;
                    }
                }
            }
            Ok(None) => break,
            Err(error) => {
                emit_protocol_error(&stdout_tx, format!("stdin read failed: {error}")).await;
                break;
            }
        }
    }
}

fn parse_rpc_command(value: &Value) -> Result<RpcInputCommand, String> {
    let command_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing command type".to_string())?;

    match command_type {
        "prompt" => Ok(RpcInputCommand::Prompt(required_rpc_content(value)?)),
        "cancel" => Ok(RpcInputCommand::Cancel),
        "steer" => Ok(RpcInputCommand::Steer(required_rpc_content(value)?)),
        "followup" => Ok(RpcInputCommand::FollowUp(required_rpc_content(value)?)),
        other => Err(format!("unknown command type: {other}")),
    }
}

fn required_rpc_content(value: &Value) -> Result<String, String> {
    value
        .get("content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "missing string field: content".to_string())
}

async fn deliver_ui_response(value: Value, pending_ui: &UiResponseMap) -> Result<(), String> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "ui_response missing id".to_string())?
        .to_string();
    let result = value.get("result").cloned().unwrap_or(Value::Null);

    let response_tx = pending_ui
        .lock()
        .await
        .remove(&id)
        .ok_or_else(|| format!("unknown ui_response id: {id}"))?;

    response_tx
        .send(result)
        .map_err(|_| format!("failed to deliver ui_response: {id}"))
}

async fn forward_rpc_events(mut handle: AgentHandle, stdout_tx: mpsc::Sender<Value>) {
    let mut runtime_state = RuntimeStateAccumulator::new("rpc");
    let mut sequence = 0_u64;
    while let Some(event) = handle.event_rx.recv().await {
        sequence += 1;
        let runtime_event = event.to_runtime_event("rpc", sequence);
        runtime_state.apply(&runtime_event);
        let _ = stdout_tx
            .send(rpc_agent_event_to_json_with_runtime(
                &event,
                &runtime_event,
                &runtime_state.snapshot(),
            ))
            .await;
    }
}

#[cfg(test)]
fn rpc_agent_event_to_json(event: &AgentEvent) -> Value {
    let runtime_event = event.to_runtime_event("rpc", 0);
    let mut runtime_state = RuntimeStateAccumulator::new("rpc");
    runtime_state.apply(&runtime_event);
    rpc_agent_event_to_json_with_runtime(event, &runtime_event, &runtime_state.snapshot())
}

fn rpc_agent_event_to_json_with_runtime(
    event: &AgentEvent,
    runtime_event: &imp_core::runtime::RuntimeEvent,
    runtime_state: &imp_core::runtime::RuntimeStateSnapshot,
) -> Value {
    let mut value = rpc_agent_event_legacy_json(event);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "runtime_event".into(),
            serde_json::to_value(runtime_event).unwrap_or(Value::Null),
        );
        object.insert(
            "runtime_state".into(),
            serde_json::to_value(runtime_state).unwrap_or(Value::Null),
        );
    }
    value
}

fn rpc_agent_event_legacy_json(event: &AgentEvent) -> Value {
    match event {
        AgentEvent::AgentStart { model, timestamp } => json!({
            "type": "agent_start",
            "model": model,
            "timestamp": timestamp,
        }),
        AgentEvent::AgentEnd {
            usage,
            cost,
            status,
        } => json!({
            "type": "agent_end",
            "usage": usage,
            "cost": cost,
            "status": status,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cache_read_tokens": usage.cache_read_tokens,
            "cache_write_tokens": usage.cache_write_tokens,
            "cost_total": cost.total,
        }),
        AgentEvent::TurnStart { index } => json!({ "type": "turn_start", "index": index }),
        AgentEvent::TurnAssessment { index, assessment } => json!({
            "type": "turn_assessment",
            "index": index,
            "assessment": next_action_assessment_to_json(assessment),
        }),
        AgentEvent::TurnEnd { index, message, .. } => {
            json!({ "type": "turn_end", "index": index, "message": message })
        }
        AgentEvent::MessageStart { message } => {
            json!({ "type": "message_start", "message": message })
        }
        AgentEvent::MessageDelta { delta } => rpc_stream_event_to_json(delta),
        AgentEvent::MessageEnd { message } => json!({ "type": "message_end", "message": message }),
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => json!({
            "type": "tool_execution_start",
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "args": args,
        }),
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            provenance,
        } => json!({
            "type": "tool_execution_end",
            "tool_call_id": tool_call_id,
            "tool_name": result.tool_name,
            "is_error": result.is_error,
            "content": result.content,
            "details": result.details,
            "timestamp": result.timestamp,
            "provenance": provenance,
        }),
        AgentEvent::Timing { timing } => json!({
            "type": "timing",
            "turn": timing.turn,
            "stage": timing.stage.as_str(),
            "since_turn_start_ms": timing.since_turn_start_ms,
            "since_llm_request_start_ms": timing.since_llm_request_start_ms,
            "duration_ms": timing.duration_ms,
            "label": timing.label,
            "success": timing.success,
        }),
        AgentEvent::RecoveryCheckpoint { checkpoint } => json!({
            "type": "recovery_checkpoint",
            "checkpoint": checkpoint,
        }),
        AgentEvent::Warning { message } => {
            json!({ "type": "warning", "message": message })
        }
        AgentEvent::WorktreeCreated { metadata } => json!({
            "type": "worktree_created",
            "metadata": metadata,
        }),
        AgentEvent::WorktreeDiffCaptured { metadata } => json!({
            "type": "worktree_diff_captured",
            "metadata": metadata,
        }),
        AgentEvent::WorktreeCloseout { result } => json!({
            "type": "worktree_closeout",
            "result": result,
        }),
        AgentEvent::EvidenceWritten { path } => json!({
            "type": "evidence_written",
            "path": path.display().to_string(),
        }),
        AgentEvent::WorkflowControllerSnapshot { snapshot } => json!({
            "type": "workflow_controller_snapshot",
            "snapshot": snapshot,
        }),
        AgentEvent::VerificationStarted { gate } => json!({
            "type": "verification_started",
            "gate": gate,
        }),
        AgentEvent::VerificationCompleted {
            gate,
            closeout_effect,
        } => json!({
            "type": "verification_completed",
            "gate": gate,
            "closeout_effect": closeout_effect,
        }),
        AgentEvent::PolicyChecked { record } => json!({
            "type": "policy_checked",
            "record": record,
        }),
        AgentEvent::Error { error } => json!({ "type": "error", "error": error }),
        AgentEvent::ToolOutputDelta { tool_call_id, text } => {
            json!({ "type": "tool_output_delta", "tool_call_id": tool_call_id, "text": text })
        }
    }
}

fn next_action_assessment_to_json(assessment: &imp_core::agent::NextActionAssessment) -> Value {
    let chosen_action = match &assessment.chosen_action {
        imp_core::agent::NextActionDebugView::Continue { prompt, reason } => json!({
            "kind": "continue",
            "prompt": prompt,
            "reason": reason,
        }),
        imp_core::agent::NextActionDebugView::Stop { reason } => json!({
            "kind": "stop",
            "reason": reason,
        }),
    };

    json!({
        "runtime": {
            "repeated_action": assessment.runtime.repeated_action,
            "execution_stop_reason": assessment.runtime.execution_stop_reason,
            "work_completed": assessment.runtime.work_completed,
            "execution_debt": assessment.runtime.execution_debt,
            "execution_evidence": assessment.runtime.execution_evidence,
            "planning_only_progress": assessment.runtime.planning_only_progress,
        },
        "workflow": {
            "stop_reason": assessment.workflow.stop_reason,
        },
        "text_fallback": {
            "planner_stop_reason": assessment.text_fallback.planner_stop_reason,
            "execution_stop_reason": assessment.text_fallback.execution_stop_reason,
        },
        "continue_recommendation": assessment.continue_recommendation.as_ref().map(|recommendation| json!({
            "prompt": recommendation.prompt,
            "reason": recommendation.reason,
        })),
        "chosen_action": chosen_action,
    })
}

fn rpc_stream_event_to_json(event: &StreamEvent) -> Value {
    match event {
        StreamEvent::MessageStart { model } => json!({ "type": "message_start", "model": model }),
        StreamEvent::TextDelta { text } => json!({ "type": "text_delta", "text": text }),
        StreamEvent::ThinkingDelta { text } => json!({ "type": "thinking_delta", "text": text }),
        StreamEvent::ToolCall {
            id,
            name,
            arguments,
        } => json!({
            "type": "tool_call",
            "id": id,
            "name": name,
            "arguments": arguments,
        }),
        StreamEvent::MessageEnd { message } => json!({ "type": "message_end", "message": message }),
        StreamEvent::Error { error } => json!({ "type": "stream_error", "error": error }),
    }
}

async fn emit_protocol_error(stdout_tx: &mpsc::Sender<Value>, error: impl Into<String>) {
    let _ = stdout_tx
        .send(json!({
            "type": "protocol_error",
            "error": error.into(),
        }))
        .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrintOutputMode {
    Text,
    Json,
}

impl PrintOutputMode {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" | "human" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!("unknown --output mode `{other}`; use text or json")),
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct PrintJsonOutcome {
    status: String,
    final_text: String,
    policy_violations: Vec<PrintPolicyViolation>,
    tool_calls: Vec<PrintToolCall>,
    usage: Option<PrintUsage>,
    cost: Option<PrintCost>,
}

#[derive(Debug, Serialize)]
struct PrintPolicyViolation {
    tool: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct PrintToolCall {
    tool: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct PrintUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Serialize)]
struct PrintCost {
    total: f64,
}

async fn run_print_mode(cli: &Cli, prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut startup_timer = StartupTimer::new(cli.verbose);
    emit_startup_timing(&mut startup_timer, StartupStage::ProcessStart);
    let cwd = std::env::current_dir()?;
    emit_startup_timing(&mut startup_timer, StartupStage::CwdResolved);
    let config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;
    emit_startup_timing(&mut startup_timer, StartupStage::ConfigResolved);

    emit_startup_timing(&mut startup_timer, StartupStage::ModelRegistryReady);
    emit_startup_timing(&mut startup_timer, StartupStage::AuthLoaded);
    emit_startup_timing(&mut startup_timer, StartupStage::ModelResolved);
    emit_startup_timing(&mut startup_timer, StartupStage::ProviderReady);
    emit_startup_timing(&mut startup_timer, StartupStage::ApiKeyResolved);

    let session_choice = if cli.no_session {
        SessionChoice::InMemory
    } else if cli.cont {
        SessionChoice::Continue
    } else if let Some(ref path) = cli.session {
        SessionChoice::Open(path.clone())
    } else {
        SessionChoice::New
    };

    let mut run_policy = imp_core::policy::RunPolicy::default();
    for tool in &cli.allow_tools {
        run_policy = run_policy.allow_tool(tool);
    }
    for tool in &cli.deny_tools {
        run_policy = run_policy.deny_tool(tool);
    }
    for pattern in &cli.allow_writes {
        run_policy = run_policy.allow_write(pattern);
    }
    for pattern in &cli.deny_writes {
        run_policy = run_policy.deny_write(pattern);
    }
    let mut options = SessionOptions {
        cwd: cwd.clone(),
        model: cli.model.clone(),
        provider: cli.provider.clone(),
        api_key: cli.api_key.clone(),
        role: cli.role.clone(),
        thinking: cli
            .thinking
            .as_ref()
            .map(|thinking| parse_thinking_level(thinking)),
        max_turns: cli.max_turns.or(config.max_turns),
        autonomy_mode: cli.autonomy,
        verification_gates: cli_verification_gates(&cli.verify),
        max_tokens: cli.max_tokens.or(config.max_tokens),
        system_prompt: cli.system_prompt.clone(),
        no_tools: cli.no_tools,
        run_policy,
        session: session_choice,
        ..Default::default()
    };
    emit_startup_timing(&mut startup_timer, StartupStage::SessionReady);

    if !cli.no_tools {
        options.lua_loader = build_lua_loader(false, std::env::current_dir().unwrap_or_default());
    }

    let mut session = ImpSession::create(options)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    emit_startup_timing(&mut startup_timer, StartupStage::AgentBuilt);

    emit_startup_timing(&mut startup_timer, StartupStage::PromptReady);
    session
        .prompt(prompt)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    emit_startup_timing(&mut startup_timer, StartupStage::RunLoopStarted);

    let mut printed_trailing_newline = false;

    let print_output_mode = PrintOutputMode::parse(&cli.output)?;
    let json_output = print_output_mode == PrintOutputMode::Json;
    let mut json_outcome = PrintJsonOutcome {
        status: "done".to_string(),
        ..Default::default()
    };
    let mut active_tool: Option<String> = None;

    while let Some(event) = session.recv_event().await {
        match event {
            AgentEvent::MessageDelta { delta } => match delta {
                StreamEvent::TextDelta { text } => {
                    if json_output {
                        json_outcome.final_text.push_str(&text);
                    } else {
                        print!("{text}");
                        printed_trailing_newline = false;
                    }
                }
                StreamEvent::ThinkingDelta { text } => {
                    if !json_output {
                        eprint!("{text}")
                    }
                }
                _ => {}
            },
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } if !cli.no_tools => {
                active_tool = Some(tool_name.clone());
                let summary = match tool_name.as_str() {
                    "bash" => args
                        .get("command")
                        .and_then(|v| v.as_str())
                        .map(|c| truncate_chars_with_suffix(c, 60, "…"))
                        .unwrap_or_default(),
                    "read" | "write" | "edit" => args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    "scan" => args
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                };
                if !json_output {
                    if summary.is_empty() {
                        eprintln!("[tool: {tool_name}]");
                    } else {
                        eprintln!("[tool: {tool_name} {summary}]");
                    }
                }
            }
            AgentEvent::ToolExecutionEnd { result, .. } if !cli.no_tools => {
                let tool_name = active_tool.take().unwrap_or_else(|| "unknown".to_string());
                let status = if result.is_error { "error" } else { "ok" }.to_string();
                let text: String = result
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if json_output {
                    if result.is_error && text.contains("run policy") {
                        json_outcome.status = "policy_denied".to_string();
                        json_outcome.policy_violations.push(PrintPolicyViolation {
                            tool: tool_name.clone(),
                            reason: text.clone(),
                        });
                    }
                    json_outcome.tool_calls.push(PrintToolCall {
                        tool: tool_name,
                        status,
                    });
                } else if result.is_error && !text.is_empty() {
                    eprintln!("[error: {}]", truncate_chars_with_suffix(&text, 100, ""));
                }
            }
            AgentEvent::TurnEnd { .. } => {
                if !json_output && !printed_trailing_newline {
                    println!();
                    printed_trailing_newline = true;
                }
            }
            AgentEvent::Error { error } => {
                json_outcome.status = "failed".to_string();
                if json_output {
                    if !json_outcome.final_text.is_empty() {
                        json_outcome.final_text.push('\n');
                    }
                    json_outcome
                        .final_text
                        .push_str(&format_error_for_display(&error));
                } else {
                    eprintln!("Error: {}", format_error_for_display(&error));
                }
            }
            AgentEvent::Timing { timing } => {
                if cli.verbose {
                    eprintln!("{}", format_timing_event(&timing));
                }
            }
            AgentEvent::AgentEnd { usage, cost, .. } => {
                if json_output {
                    json_outcome.usage = Some(PrintUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    });
                    json_outcome.cost = Some(PrintCost { total: cost.total });
                } else {
                    eprintln!(
                        "\n[tokens: ↑{} ↓{} | cost: ${:.4}]",
                        usage.input_tokens, usage.output_tokens, cost.total
                    );
                }
            }
            _ => {}
        }
    }

    session
        .wait()
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    if json_output {
        println!("{}", serde_json::to_string(&json_outcome)?);
    }

    Ok(())
}

fn parse_thinking_level_strict(raw: &str) -> Option<ThinkingLevel> {
    match raw.trim().to_lowercase().as_str() {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::XHigh),
        _ => None,
    }
}

fn tool_output_display_label(display: ToolOutputDisplay) -> &'static str {
    match display {
        ToolOutputDisplay::Full => "full",
        ToolOutputDisplay::Compact => "compact",
        ToolOutputDisplay::Collapsed => "collapsed",
    }
}

fn parse_tool_output_display(raw: &str) -> Option<ToolOutputDisplay> {
    match raw.trim().to_lowercase().as_str() {
        "full" => Some(ToolOutputDisplay::Full),
        "compact" => Some(ToolOutputDisplay::Compact),
        "collapsed" => Some(ToolOutputDisplay::Collapsed),
        _ => None,
    }
}

fn web_search_provider_label(provider: Option<SearchProvider>) -> &'static str {
    match provider {
        Some(provider) => provider.name(),
        None => "none",
    }
}

fn prompt_input_line(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    eprint!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    if bytes == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed").into());
    }
    Ok(input.trim().to_string())
}

fn prompt_optional_input_line(prompt: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    eprint!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    let bytes = io::stdin().read_line(&mut input)?;
    if bytes == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim().to_string()))
}

fn save_user_config(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = imp_core::storage::global_config_path();
    config
        .save(&path)
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    Ok(path)
}

fn print_settings_summary(config: &Config, config_path: &Path) {
    println!("Settings ({})", config_path.display());
    println!("================");
    println!(
        "1. model               {}",
        config.model.as_deref().unwrap_or("(unset)")
    );
    println!(
        "2. thinking            {}",
        config
            .thinking
            .map(thinking_level_label)
            .unwrap_or("(unset)")
    );
    println!(
        "3. max_tokens          {}",
        config
            .max_tokens
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(unset)".to_string())
    );
    println!(
        "4. max_turns           {}",
        config
            .max_turns
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(unset)".to_string())
    );
    println!(
        "5. tool_output         {}",
        tool_output_display_label(config.ui.tool_output)
    );
    println!(
        "6. web_search_provider {}",
        web_search_provider_label(config.web.search_provider)
    );
    println!("s. save and exit");
    println!("q. quit without saving");
}

fn run_settings_mode() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let config_path = imp_core::storage::global_config_path();
    let mut config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;

    loop {
        println!();
        print_settings_summary(&config, &config_path);
        let Some(choice) = prompt_optional_input_line("Select field> ")? else {
            println!();
            return Ok(());
        };

        match choice.trim() {
            "1" => {
                let value = prompt_input_line("model> ")?;
                if value.is_empty() {
                    config.model = None;
                    println!("Cleared model.");
                } else {
                    config.model = Some(value);
                    println!("Updated model.");
                }
            }
            "2" => {
                let value = prompt_input_line("thinking [off|minimal|low|medium|high|xhigh]> ")?;
                if value.is_empty() {
                    config.thinking = None;
                    println!("Cleared thinking level.");
                } else if let Some(level) = parse_thinking_level_strict(&value) {
                    config.thinking = Some(level);
                    println!("Updated thinking level.");
                } else {
                    println!("Unknown thinking level: {value}");
                }
            }
            "3" => {
                let value = prompt_input_line("max_tokens> ")?;
                if value.is_empty() {
                    config.max_tokens = None;
                    println!("Cleared max_tokens.");
                } else if let Ok(parsed) = value.parse::<u32>() {
                    config.max_tokens = Some(parsed.max(1));
                    println!("Updated max_tokens.");
                } else {
                    println!("Expected a positive integer.");
                }
            }
            "4" => {
                let value = prompt_input_line("max_turns> ")?;
                if value.is_empty() {
                    config.max_turns = None;
                    println!("Cleared max_turns.");
                } else if let Ok(parsed) = value.parse::<u32>() {
                    config.max_turns = Some(parsed.max(1));
                    println!("Updated max_turns.");
                } else {
                    println!("Expected a positive integer.");
                }
            }
            "5" => {
                let value = prompt_input_line("tool_output [full|compact|collapsed]> ")?;
                if let Some(display) = parse_tool_output_display(&value) {
                    config.ui.tool_output = display;
                    println!("Updated tool output display.");
                } else {
                    println!("Expected one of: full, compact, collapsed.");
                }
            }
            "6" => {
                let value =
                    prompt_input_line("web_search_provider [none|tavily|exa|linkup|perplexity]> ")?;
                match value.trim().to_lowercase().as_str() {
                    "" | "none" => {
                        config.web.search_provider = None;
                        println!("Cleared web search provider.");
                    }
                    "tavily" => {
                        config.web.search_provider = Some(SearchProvider::Tavily);
                        println!("Updated web search provider.");
                    }
                    "exa" => {
                        config.web.search_provider = Some(SearchProvider::Exa);
                        println!("Updated web search provider.");
                    }
                    "linkup" => {
                        config.web.search_provider = Some(SearchProvider::Linkup);
                        println!("Updated web search provider.");
                    }
                    "perplexity" => {
                        config.web.search_provider = Some(SearchProvider::Perplexity);
                        println!("Updated web search provider.");
                    }
                    _ => println!("Expected one of: none, tavily, exa, linkup, perplexity."),
                }
            }
            "s" | "save" => {
                let saved_path = save_user_config(&config)?;
                println!("Saved settings to {}", saved_path.display());
                return Ok(());
            }
            "q" | "quit" => {
                println!("Discarded settings changes.");
                return Ok(());
            }
            other => {
                println!("Unknown selection: {other}");
            }
        }
    }
}

fn thinking_level_label(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

async fn run_view_mode(_cli: &Cli, area: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let session_dir = imp_core::storage::global_sessions_dir();

    match area.unwrap_or("sessions") {
        "sessions" => {
            let sessions = SessionManager::list(&session_dir)?;
            if sessions.is_empty() {
                println!("No saved sessions found.");
                return Ok(());
            }

            println!("Sessions\n========");
            for (idx, session) in sessions.iter().enumerate().take(20) {
                let title = session.title(72).unwrap_or_else(|| session.id.clone());
                let project = session.cwd.clone();
                println!("{}. {}", idx + 1, title);
                println!("   id: {}", session.id);
                println!("   project: {}", project);
                println!("   path: {}", session.path.display());
                println!("   messages: {}", session.message_count);
                if let Some(summary) = &session.summary {
                    println!(
                        "   summary: {}",
                        truncate_chars_with_suffix(summary, 120, "…")
                    );
                }
            }
            if sessions.len() > 20 {
                println!("… {} more session(s)", sessions.len() - 20);
            }
            Ok(())
        }
        "tree" => {
            let session =
                SessionManager::continue_recent(&cwd, &session_dir)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "No recent session found for this working directory.",
                    )
                })?;
            let tree = session.get_tree();
            if tree.is_empty() {
                println!("No session history yet.");
                return Ok(());
            }

            println!("Session tree\n============");
            print_tree_nodes(&tree, 0);
            Ok(())
        }
        "logs" => {
            let session =
                SessionManager::continue_recent(&cwd, &session_dir)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "No recent session found for this working directory.",
                    )
                })?;
            println!("Session log\n===========");
            for entry in session.entries().iter().rev().take(40).rev() {
                println!("{}", summarize_session_entry(entry));
            }
            Ok(())
        }
        "checkpoints" => {
            let session =
                SessionManager::continue_recent(&cwd, &session_dir)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "No recent session found for this working directory.",
                    )
                })?;
            let checkpoints = session.checkpoint_records();
            if checkpoints.is_empty() {
                println!("No checkpoints recorded in the most recent session for this working directory.");
                return Ok(());
            }

            println!("Checkpoints\n===========");
            for checkpoint in checkpoints {
                let label = checkpoint
                    .label
                    .as_deref()
                    .map(|label| format!(" — {label}"))
                    .unwrap_or_default();
                println!(
                    "- {}{} ({} file{})",
                    checkpoint.checkpoint_id,
                    label,
                    checkpoint.files.len(),
                    if checkpoint.files.len() == 1 { "" } else { "s" }
                );
                for file in checkpoint.files.iter().take(8) {
                    println!("    {file}");
                }
                if checkpoint.files.len() > 8 {
                    println!("    … {} more", checkpoint.files.len() - 8);
                }
            }
            Ok(())
        }
        other => {
            eprintln!(
                "Unknown viewer area: {other}. Use one of: sessions, tree, logs, checkpoints."
            );
            Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown viewer area").into())
        }
    }
}

fn print_tree_nodes(nodes: &[imp_core::session::TreeNode], depth: usize) {
    for node in nodes {
        let indent = "  ".repeat(depth);
        let summary = match &node.entry {
            SessionEntry::Header { cwd, .. } => format!("header {cwd}"),
            SessionEntry::SessionMeta { name, summary, .. } => format!(
                "session-meta {}{}",
                name.as_deref().unwrap_or("(unnamed)"),
                summary
                    .as_deref()
                    .map(|s| format!(" — {}", truncate_chars_with_suffix(s, 60, "…")))
                    .unwrap_or_default()
            ),
            SessionEntry::Message { message, .. } => summarize_message_for_view(message),
            SessionEntry::Compaction { summary, .. } => {
                format!(
                    "compaction {}",
                    truncate_chars_with_suffix(summary, 60, "…")
                )
            }
            SessionEntry::Label { label, .. } => format!("label {label}"),
            SessionEntry::Custom { custom_type, .. } => format!("custom {custom_type}"),
        };
        println!("{indent}- {summary}");
        print_tree_nodes(&node.children, depth + 1);
    }
}

fn summarize_message_for_view(message: &Message) -> String {
    let text_content = |message: &Message| -> Option<String> {
        let blocks = match message {
            Message::User(user) => &user.content,
            Message::Assistant(assistant) => &assistant.content,
            Message::ToolResult(result) => &result.content,
        };
        blocks.iter().find_map(|block| match block {
            imp_llm::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
    };

    match message {
        Message::User(user) => format!(
            "user {}",
            truncate_chars_with_suffix(
                &text_content(message).unwrap_or_else(|| {
                    user.content
                        .iter()
                        .filter_map(|block| match block {
                            imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
                80,
                "…"
            )
        ),
        Message::Assistant(_) => format!(
            "assistant {}",
            truncate_chars_with_suffix(&text_content(message).unwrap_or_default(), 80, "…")
        ),
        Message::ToolResult(result) => format!(
            "tool-result {}",
            truncate_chars_with_suffix(
                &text_content(message).unwrap_or_else(|| result.tool_call_id.clone()),
                80,
                "…"
            )
        ),
    }
}

fn summarize_session_entry(entry: &SessionEntry) -> String {
    match entry {
        SessionEntry::Header { cwd, .. } => format!("header cwd={cwd}"),
        SessionEntry::SessionMeta { name, summary, .. } => format!(
            "session-meta name={} summary={}",
            name.as_deref().unwrap_or("(unnamed)"),
            summary.as_deref().unwrap_or("(none)")
        ),
        SessionEntry::Message { message, .. } => summarize_message_for_view(message),
        SessionEntry::Compaction { summary, .. } => {
            format!(
                "compaction {}",
                truncate_chars_with_suffix(summary, 100, "…")
            )
        }
        SessionEntry::Label { label, .. } => format!("label {label}"),
        SessionEntry::Custom { custom_type, .. } => format!("custom {custom_type}"),
    }
}

async fn run_interactive(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let interactive_result = async {
        let cwd = std::env::current_dir()?;
        let config = Config::resolve(&imp_core::storage::global_root(), Some(&cwd))?;

        let registry = ModelRegistry::with_builtins();

        let session = if cli.no_session {
            SessionManager::in_memory()
        } else if cli.cont {
            // Continue most recent session
            match SessionManager::continue_recent(&cwd, &imp_core::storage::global_sessions_dir())?
            {
                Some(session) => session,
                None => SessionManager::new(&cwd, &imp_core::storage::global_sessions_dir())?,
            }
        } else if let Some(ref path) = cli.session {
            SessionManager::open(path)?
        } else {
            // New persistent session
            SessionManager::new(&cwd, &imp_core::storage::global_sessions_dir())?
        };

        let mut runner =
            imp_tui::interactive::InteractiveRunner::new(config, session, registry, cwd)?;

        // Apply CLI overrides
        if let Some(ref model) = cli.model {
            runner.app_mut().model_name = model.clone();
        }
        if let Some(ref thinking) = cli.thinking {
            runner.app_mut().thinking_level = parse_thinking_level(thinking);
        }

        runner.run_guarded().await.map_err(Into::into)
    }
    .await;

    interactive_result
}

/// Expand @file arguments into file content blocks.
/// Returns a string with each file's content wrapped in XML-like tags.
fn expand_file_args(args: &[String]) -> String {
    let mut parts = Vec::new();
    for arg in args {
        if let Some(path_str) = arg.strip_prefix('@') {
            let path = std::path::Path::new(path_str);
            // Expand ~ in @~/path
            let resolved = if let Some(rest) = path_str.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(rest)
                } else {
                    path.to_path_buf()
                }
            } else {
                path.to_path_buf()
            };
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    parts.push(format!(
                        "<file path=\"{}\">\n{}\n</file>",
                        resolved.display(),
                        content.trim_end()
                    ));
                }
                Err(e) => {
                    eprintln!("Warning: cannot read {}: {e}", resolved.display());
                }
            }
        }
    }
    parts.join("\n\n")
}

fn prompt_args(args: &[String]) -> Vec<&str> {
    args.iter()
        .filter(|arg| !arg.starts_with('@'))
        .map(String::as_str)
        .collect()
}

/// Build the full prompt from user text, @file context, and stdin.
fn build_full_prompt(prompt: &str, file_context: &str, stdin: &Option<String>) -> String {
    let mut parts = Vec::new();
    if !file_context.is_empty() {
        parts.push(file_context.to_string());
    }
    if let Some(ref content) = stdin {
        parts.push(format!("<stdin>\n{}\n</stdin>", content.trim_end()));
    }
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.join("\n\n")
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use imp_llm::auth::{OAuthCredential, StoredCredential};
    use imp_llm::stream::StreamEvent;
    use serde_json::json;

    /// Helper: build a minimal Cli struct with defaults for testing.
    fn default_cli() -> Cli {
        Cli {
            print: None,
            provider: None,
            model: None,
            role: None,
            thinking: None,
            api_key: None,
            cont: false,
            resume: false,
            session: None,
            no_session: false,
            tools: None,
            allow_tools: Vec::new(),
            deny_tools: Vec::new(),
            allow_writes: Vec::new(),
            deny_writes: Vec::new(),
            no_tools: false,
            system_prompt: None,
            mode: "interactive".to_string(),
            output: "text".to_string(),
            runtime_json: false,
            autonomy: None,
            verify: Vec::new(),
            max_turns: None,
            max_tokens: None,
            verbose: false,
            list_models: false,
            args: Vec::new(),
            command: None,
        }
    }

    fn empty_auth_store() -> AuthStore {
        AuthStore::new(std::path::PathBuf::from("auth.json"))
    }

    #[test]
    fn prompt_args_excludes_file_context_args() {
        let args = vec![
            "help".to_string(),
            "me".to_string(),
            "@README.md".to_string(),
            "install".to_string(),
            "utop".to_string(),
        ];

        assert_eq!(prompt_args(&args), vec!["help", "me", "install", "utop"]);
    }

    #[test]
    fn build_full_prompt_keeps_bare_prompt_after_file_context() {
        let prompt = build_full_prompt("help me install utop", "<file>ctx</file>", &None);

        assert_eq!(prompt, "<file>ctx</file>\n\nhelp me install utop");
    }

    #[test]
    fn cli_parses_autonomy_mode_flag() {
        let cli = Cli::try_parse_from(["imp", "--autonomy", "allow-all-local", "fix it"])
            .expect("parse autonomy flag");
        assert_eq!(cli.autonomy, Some(AutonomyMode::AllowAllLocal));
        assert_eq!(cli.args, vec!["fix it".to_string()]);
    }

    #[test]
    fn cli_rejects_unknown_autonomy_mode() {
        let err = match Cli::try_parse_from(["imp", "--autonomy", "dangerous", "fix it"]) {
            Ok(_) => panic!("unknown autonomy mode should fail"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn cli_parses_verify_command_gates() {
        let cli = Cli::try_parse_from([
            "imp",
            "--verify",
            "cargo test -p imp-core",
            "--verify",
            "cargo fmt --check",
            "fix it",
        ])
        .expect("parse verify gates");
        let gates = cli_verification_gates(&cli.verify);
        assert_eq!(gates.len(), 2);
        assert_eq!(gates[0].id, "cli-verify-1");
        assert_eq!(
            gates[0]
                .command
                .as_ref()
                .map(|command| command.command.as_str()),
            Some("cargo test -p imp-core")
        );
        assert_eq!(gates[1].id, "cli-verify-2");
    }

    #[test]
    fn cli_parses_workflow_commands() {
        let cli = Cli::try_parse_from([
            "imp",
            "workflow",
            "validate",
            "audit-and-repair-workflows",
            "--mode",
            "draft",
        ])
        .expect("parse workflow validate");
        match cli.command {
            Some(Commands::Workflow {
                command: WorkflowCommand::Validate(args),
            }) => {
                assert_eq!(args.id.as_deref(), Some("audit-and-repair-workflows"));
                assert!(matches!(args.mode, WorkflowValidationModeArg::Draft));
            }
            other => panic!("expected workflow validate command, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "imp",
            "workflow",
            "update",
            "audit-and-repair-workflows",
            "status",
            "done",
            "--reason",
            "verified",
        ])
        .expect("parse workflow update");
        match cli.command {
            Some(Commands::Workflow {
                command: WorkflowCommand::Update(args),
            }) => {
                assert_eq!(args.id, "audit-and-repair-workflows");
                assert_eq!(args.path, "status");
                assert_eq!(args.value, "done");
                assert_eq!(args.reason, "verified");
            }
            other => panic!("expected workflow update command, got {other:?}"),
        }
    }

    #[test]
    fn cli_workflow_rejects_invalid_validation_mode() {
        let err = match Cli::try_parse_from(["imp", "workflow", "validate", "--mode", "loose"]) {
            Ok(_) => panic!("invalid workflow validation mode should fail"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
    }

    #[test]
    fn cli_treats_old_run_workflow_flags_as_prompt_args() {
        let cli = Cli::try_parse_from(["imp", "run", "5.1", "--defer-verify"])
            .expect("legacy native work run flags are no longer a subcommand");
        assert!(cli.command.is_none());
        assert_eq!(cli.args, vec!["run", "5.1", "--defer-verify"]);
    }

    #[test]
    fn parse_tool_output_display_accepts_known_values() {
        assert_eq!(
            parse_tool_output_display("full"),
            Some(ToolOutputDisplay::Full)
        );
        assert_eq!(
            parse_tool_output_display("compact"),
            Some(ToolOutputDisplay::Compact)
        );
        assert_eq!(
            parse_tool_output_display("collapsed"),
            Some(ToolOutputDisplay::Collapsed)
        );
        assert_eq!(parse_tool_output_display("mystery"), None);
    }

    #[test]
    fn web_search_provider_label_formats_none_and_provider_names() {
        assert_eq!(web_search_provider_label(None), "none");
        assert_eq!(web_search_provider_label(Some(SearchProvider::Exa)), "exa");
    }

    #[test]
    fn resolve_install_destination_prefers_active_user_imp_path() {
        let home = PathBuf::from("/Users/test");
        let path = Some(OsString::from(
            "/Users/test/bin:/Users/test/.cargo/bin:/usr/bin",
        ));
        let active_imp = Some(PathBuf::from("/Users/test/bin/imp"));

        let dest = resolve_install_destination(&home, path, active_imp, None);
        assert_eq!(dest, PathBuf::from("/Users/test/bin/imp"));
    }
    #[test]
    fn resolve_install_destination_falls_back_to_path_preference_when_imp_missing() {
        let home = PathBuf::from("/Users/test");
        let path = Some(OsString::from(
            "/Users/test/bin:/Users/test/.cargo/bin:/usr/bin",
        ));

        let dest = resolve_install_destination(&home, path, None, None);
        assert_eq!(dest, PathBuf::from("/Users/test/bin/imp"));
    }
    #[test]
    fn resolve_install_destination_uses_cargo_bin_when_no_user_bin_is_on_path() {
        let home = PathBuf::from("/Users/test");
        let path = Some(OsString::from("/usr/local/bin:/usr/bin"));

        let dest = resolve_install_destination(&home, path, None, None);
        assert_eq!(dest, PathBuf::from("/Users/test/.cargo/bin/imp"));
    }
    #[test]
    fn parse_thinking_level_strict_rejects_unknown_values() {
        assert_eq!(
            parse_thinking_level_strict("medium"),
            Some(ThinkingLevel::Medium)
        );
        assert_eq!(parse_thinking_level_strict("turbo"), None);
    }

    #[test]
    fn thinking_level_label_matches_expected_strings() {
        assert_eq!(thinking_level_label(ThinkingLevel::Off), "off");
        assert_eq!(thinking_level_label(ThinkingLevel::Minimal), "minimal");
        assert_eq!(thinking_level_label(ThinkingLevel::Low), "low");
        assert_eq!(thinking_level_label(ThinkingLevel::Medium), "medium");
        assert_eq!(thinking_level_label(ThinkingLevel::High), "high");
        assert_eq!(thinking_level_label(ThinkingLevel::XHigh), "xhigh");
    }

    // ── parse_thinking_level ───────────────────────────────────────

    #[test]
    fn parse_thinking_level_all_variants() {
        assert!(matches!(parse_thinking_level("off"), ThinkingLevel::Off));
        assert!(matches!(
            parse_thinking_level("minimal"),
            ThinkingLevel::Minimal
        ));
        assert!(matches!(parse_thinking_level("low"), ThinkingLevel::Low));
        assert!(matches!(
            parse_thinking_level("medium"),
            ThinkingLevel::Medium
        ));
        assert!(matches!(parse_thinking_level("high"), ThinkingLevel::High));
        assert!(matches!(
            parse_thinking_level("xhigh"),
            ThinkingLevel::XHigh
        ));
    }

    #[test]
    fn parse_thinking_level_unknown_defaults_to_off() {
        assert!(matches!(parse_thinking_level("turbo"), ThinkingLevel::Off));
        assert!(matches!(parse_thinking_level(""), ThinkingLevel::Off));
    }

    #[test]
    fn parse_thinking_level_case_insensitive() {
        assert!(matches!(parse_thinking_level("HIGH"), ThinkingLevel::High));
        assert!(matches!(
            parse_thinking_level("Medium"),
            ThinkingLevel::Medium
        ));
    }

    // ── resolve_model_and_provider ─────────────────────────────────

    #[test]
    fn resolve_model_sonnet_alias() {
        let cli = default_cli();
        let config = Config::default();
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let (model_id, provider) =
            resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
        // Default is "sonnet"
        assert!(
            model_id.contains("sonnet"),
            "expected sonnet, got {model_id}"
        );
        assert_eq!(provider, "anthropic");
    }

    #[test]
    fn resolve_model_haiku_alias() {
        let mut cli = default_cli();
        cli.model = Some("haiku".to_string());
        let config = Config::default();
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let (model_id, provider) =
            resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
        assert!(model_id.contains("haiku"), "expected haiku, got {model_id}");
        assert_eq!(provider, "anthropic");
    }

    #[test]
    fn resolve_model_unknown_alias_errors() {
        let mut cli = default_cli();
        cli.model = Some("nonexistent-xyz".to_string());
        let config = Config::default();
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let result = resolve_model_and_provider(&cli, &config, &registry, &auth_store);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown model"));
    }

    #[test]
    fn resolve_model_allows_custom_openai_model() {
        let mut cli = default_cli();
        cli.model = Some("gpt-4o".to_string());
        let config = Config::default();
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let (model_id, provider) =
            resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
        assert_eq!(model_id, "gpt-4o");
        assert_eq!(provider, "openai");
    }

    #[test]
    fn resolve_model_cli_overrides_config() {
        let mut cli = default_cli();
        cli.model = Some("haiku".to_string());
        let config = Config {
            model: Some("sonnet".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let (model_id, _) =
            resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
        assert!(
            model_id.contains("haiku"),
            "CLI --model should override config"
        );
    }

    #[test]
    fn resolve_model_cli_provider_override() {
        let mut cli = default_cli();
        cli.provider = Some("openai".to_string());
        // Use default sonnet — provider override just changes provider name
        let config = Config::default();
        let registry = ModelRegistry::with_builtins();
        let auth_store = empty_auth_store();
        let (_, provider) =
            resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
        assert_eq!(provider, "openai");
    }

    #[test]
    fn resolve_model_prefers_chatgpt_provider_when_only_oauth_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut auth_store = AuthStore::new(path);
        auth_store
            .store(
                "openai",
                StoredCredential::OAuth(OAuthCredential {
                    access_token: "oauth-token".into(),
                    refresh_token: "refresh-token".into(),
                    expires_at: imp_llm::now() + 3600,
                }),
            )
            .unwrap();

        let config = Config {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::with_builtins();

        let (model_id, provider) =
            resolve_model_and_provider(&default_cli(), &config, &registry, &auth_store).unwrap();
        assert_eq!(model_id, "gpt-5.4");
        assert_eq!(provider, "openai-codex");
    }

    #[test]
    fn resolve_model_keeps_openai_when_api_key_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut auth_store = AuthStore::new(path);
        auth_store
            .store(
                "openai",
                StoredCredential::ApiKey {
                    key: "sk-openai".into(),
                },
            )
            .unwrap();
        auth_store
            .store(
                "openai-codex",
                StoredCredential::OAuth(OAuthCredential {
                    access_token: "oauth-token".into(),
                    refresh_token: "refresh-token".into(),
                    expires_at: imp_llm::now() + 3600,
                }),
            )
            .unwrap();

        let config = Config {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::with_builtins();

        let (model_id, provider) =
            resolve_model_and_provider(&default_cli(), &config, &registry, &auth_store).unwrap();
        assert_eq!(model_id, "gpt-5.4");
        assert_eq!(provider, "openai");
    }

    #[test]
    fn resolve_custom_openai_model_does_not_switch_to_chatgpt_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let mut auth_store = AuthStore::new(path);
        auth_store
            .store(
                "openai",
                StoredCredential::OAuth(OAuthCredential {
                    access_token: "oauth-token".into(),
                    refresh_token: "refresh-token".into(),
                    expires_at: imp_llm::now() + 3600,
                }),
            )
            .unwrap();

        let config = Config {
            model: Some("gpt-4o".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::with_builtins();

        let (model_id, provider) =
            resolve_model_and_provider(&default_cli(), &config, &registry, &auth_store).unwrap();
        assert_eq!(model_id, "gpt-4o");
        assert_eq!(provider, "openai");
    }

    // ── parse_rpc_command ──────────────────────────────────────────

    #[test]
    fn parse_rpc_prompt_command() {
        let value = json!({"type": "prompt", "content": "hello"});
        let cmd = parse_rpc_command(&value).unwrap();
        assert!(matches!(cmd, RpcInputCommand::Prompt(ref s) if s == "hello"));
    }

    #[test]
    fn parse_rpc_cancel_command() {
        let value = json!({"type": "cancel"});
        let cmd = parse_rpc_command(&value).unwrap();
        assert!(matches!(cmd, RpcInputCommand::Cancel));
    }

    #[test]
    fn parse_rpc_steer_command() {
        let value = json!({"type": "steer", "content": "also do X"});
        let cmd = parse_rpc_command(&value).unwrap();
        assert!(matches!(cmd, RpcInputCommand::Steer(ref s) if s == "also do X"));
    }

    #[test]
    fn parse_rpc_followup_command() {
        let value = json!({"type": "followup", "content": "next step"});
        let cmd = parse_rpc_command(&value).unwrap();
        assert!(matches!(cmd, RpcInputCommand::FollowUp(ref s) if s == "next step"));
    }

    #[test]
    fn parse_rpc_unknown_type_errors() {
        let value = json!({"type": "bogus"});
        let result = parse_rpc_command(&value);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown command type"));
    }

    #[test]
    fn parse_rpc_missing_type_errors() {
        let value = json!({"content": "hello"});
        let result = parse_rpc_command(&value);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing command type"));
    }

    #[test]
    fn parse_rpc_prompt_missing_content_errors() {
        let value = json!({"type": "prompt"});
        let result = parse_rpc_command(&value);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing string field"));
    }

    // ── rpc_stream_event_to_json ───────────────────────────────────

    #[test]
    fn rpc_stream_event_text_delta() {
        let event = StreamEvent::TextDelta {
            text: "hello".to_string(),
        };
        let json = rpc_stream_event_to_json(&event);
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hello");
    }

    #[test]
    fn rpc_stream_event_tool_call() {
        let event = StreamEvent::ToolCall {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            arguments: json!({"command": "ls"}),
        };
        let json = rpc_stream_event_to_json(&event);
        assert_eq!(json["type"], "tool_call");
        assert_eq!(json["name"], "bash");
        assert_eq!(json["arguments"]["command"], "ls");
    }

    // ── rpc_agent_event_to_json ────────────────────────────────────

    #[test]
    fn rpc_agent_event_tool_execution_start() {
        let event = AgentEvent::ToolExecutionStart {
            tool_call_id: "call_42".to_string(),
            tool_name: "read".to_string(),
            args: json!({"path": "/tmp/test.txt"}),
        };
        let json = rpc_agent_event_to_json(&event);
        assert_eq!(json["type"], "tool_execution_start");
        assert_eq!(json["tool_name"], "read");
        assert_eq!(json["args"]["path"], "/tmp/test.txt");
    }

    #[test]
    fn rpc_agent_event_agent_end() {
        let usage = imp_llm::Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 100,
            cache_write_tokens: 50,
        };
        let cost = imp_llm::Cost {
            input: 0.003,
            output: 0.0075,
            cache_read: 0.00003,
            cache_write: 0.0001875,
            total: 0.0107175,
        };
        let event = AgentEvent::AgentEnd {
            usage,
            cost,
            status: imp_core::agent::RunFinalStatus::Done {
                reason: imp_core::agent::StopReason::WorkCompleted,
            },
        };
        let json = rpc_agent_event_to_json(&event);
        assert_eq!(json["type"], "agent_end");
        assert_eq!(json["input_tokens"], 1000);
        assert_eq!(json["output_tokens"], 500);
        assert_eq!(json["cache_read_tokens"], 100);
        assert_eq!(json["cost_total"], 0.0107175);
        assert_eq!(json["status"]["type"], "done");
        assert_eq!(json["status"]["reason"], "work_completed");
    }

    #[test]
    fn rpc_agent_event_agent_end_serializes_blocked_status() {
        let event = AgentEvent::AgentEnd {
            usage: imp_llm::Usage::default(),
            cost: imp_llm::Cost::default(),
            status: imp_core::agent::RunFinalStatus::Blocked {
                reason: imp_core::agent::StopReason::ExecutionBlocked,
                message: "verification failed".to_string(),
            },
        };
        let json = rpc_agent_event_to_json(&event);
        assert_eq!(json["type"], "agent_end");
        assert_eq!(json["status"]["type"], "blocked");
        assert_eq!(json["status"]["reason"], "execution_blocked");
        assert_eq!(json["status"]["message"], "verification failed");
    }

    #[test]
    fn rpc_agent_event_agent_end_serializes_failed_status() {
        let event = AgentEvent::AgentEnd {
            usage: imp_llm::Usage::default(),
            cost: imp_llm::Cost::default(),
            status: imp_core::agent::RunFinalStatus::Failed {
                message: "provider error".to_string(),
            },
        };
        let json = rpc_agent_event_to_json(&event);
        assert_eq!(json["type"], "agent_end");
        assert_eq!(json["status"]["type"], "failed");
        assert_eq!(json["status"]["message"], "provider error");
    }

    #[test]
    fn rpc_agent_event_timing() {
        let event = AgentEvent::Timing {
            timing: TimingEvent {
                turn: 2,
                stage: imp_core::TimingStage::FirstTextDelta,
                since_turn_start_ms: 150,
                since_llm_request_start_ms: Some(120),
                duration_ms: Some(30),
                label: Some("model".to_string()),
                success: Some(true),
            },
        };
        let json = rpc_agent_event_to_json(&event);
        assert_eq!(json["type"], "timing");
        assert_eq!(json["turn"], 2);
        assert_eq!(json["stage"], "first_text_delta");
        assert_eq!(json["since_turn_start_ms"], 150);
        assert_eq!(json["since_llm_request_start_ms"], 120);
        assert_eq!(json["duration_ms"], 30);
        assert_eq!(json["label"], "model");
        assert_eq!(json["success"], true);
    }

    #[test]
    fn startup_stage_names_are_stable() {
        assert_eq!(StartupStage::ProcessStart.as_str(), "process_start");
        assert_eq!(StartupStage::RunLoopStarted.as_str(), "run_loop_started");
    }
}

fn run_import(dry_run: bool, from: Option<&str>, auto_yes: bool) {
    use imp_core::import::{
        detect_sources, import_agents_md, import_skills, AgentSource, SkipReason,
    };

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            eprintln!("Cannot determine home directory");
            std::process::exit(1);
        }
    };

    let sources = detect_sources(&home);

    // Filter by --from if specified
    let sources: Vec<_> = if let Some(filter) = from {
        let target = match filter.to_lowercase().as_str() {
            "pi" => Some(AgentSource::Pi),
            "claude" | "claude-code" => Some(AgentSource::ClaudeCode),
            "codex" => Some(AgentSource::Codex),
            other => {
                eprintln!("Unknown agent: {other}. Use: pi, claude, codex");
                std::process::exit(1);
            }
        };
        sources
            .into_iter()
            .filter(|s| target.is_none_or(|t| s.agent == t))
            .collect()
    } else {
        sources
    };

    if sources.is_empty() {
        println!("No other agent configurations found.");
        println!("Checked: ~/.pi/agent/, ~/.claude/, ~/.codex/");
        return;
    }

    // Display what was found
    println!("Found agent configurations:\n");
    let mut total_skills = 0;
    let mut total_agents_md = 0;

    for source in &sources {
        println!(
            "  {} ({})",
            source.agent.label(),
            match source.agent {
                AgentSource::Pi => "~/.pi/agent/",
                AgentSource::ClaudeCode => "~/.claude/",
                AgentSource::Codex => "~/.codex/",
            }
        );

        if !source.skills.is_empty() {
            println!("    {} skills:", source.skills.len());
            for skill in &source.skills {
                let desc = truncate_chars_with_suffix(&skill.description, 60, "…");
                println!("      - {} — {}", skill.name, desc);
            }
            total_skills += source.skills.len();
        }

        if !source.agents_md.is_empty() {
            for md in &source.agents_md {
                println!("    {} at {}", md.kind.label(), md.path.display());
            }
            total_agents_md += source.agents_md.len();
        }

        println!();
    }

    if dry_run {
        println!("Dry run — nothing was copied.");
        println!("Run without --dry-run to import.");
        return;
    }

    if total_skills == 0 && total_agents_md == 0 {
        println!("Nothing to import.");
        return;
    }

    // Confirm unless --yes
    if !auto_yes {
        print!(
            "Import {} skills and {} instruction files into imp? [y/N] ",
            total_skills, total_agents_md
        );
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return;
        }
    }

    let imp_config = Config::user_config_dir();
    let imp_skills = imp_config.join("skills");
    // Import skills
    for source in &sources {
        if source.skills.is_empty() {
            continue;
        }

        match import_skills(&source.skills, &imp_skills) {
            Ok(result) => {
                if !result.copied.is_empty() {
                    println!(
                        "  ✓ Imported {} skills from {}:",
                        result.copied.len(),
                        source.agent.label()
                    );
                    for name in &result.copied {
                        println!("      {name}");
                    }
                }
                for (name, reason) in &result.skipped {
                    match reason {
                        SkipReason::AlreadyExists => {
                            println!("    ⊘ {name} — already exists, skipped");
                        }
                        SkipReason::CopyFailed(err) => {
                            eprintln!("    ✗ {name} — copy failed: {err}");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  ✗ Failed to import skills from {}: {e}",
                    source.agent.label()
                );
            }
        }
    }

    // Import AGENTS.md (only the first one found, if imp doesn't have one yet)
    let mut imported_agents = false;
    for source in &sources {
        for md in &source.agents_md {
            if imported_agents {
                println!(
                    "    ⊘ {} from {} — already have AGENTS.md, skipped",
                    md.kind.label(),
                    source.agent.label()
                );
                continue;
            }
            match import_agents_md(md, &imp_config) {
                Ok(Some(dest)) => {
                    println!(
                        "  ✓ Imported {} from {} → {}",
                        md.kind.label(),
                        source.agent.label(),
                        dest.display()
                    );
                    imported_agents = true;
                }
                Ok(None) => {
                    println!("    ⊘ AGENTS.md already exists in imp config, skipped");
                    imported_agents = true;
                }
                Err(e) => {
                    eprintln!(
                        "  ✗ Failed to import {} from {}: {e}",
                        md.kind.label(),
                        source.agent.label()
                    );
                }
            }
        }
    }

    println!("\nDone. Skills are in {}", imp_skills.display());
}
