use std::collections::{HashMap, VecDeque};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

mod import;
mod local_install;
mod models;
mod provider_secrets;
mod secrets;
mod setup;
mod startup_timing;
use local_install::run_install_local;
use startup_timing::{emit_startup_timing, StartupTimer};
pub use startup_timing::{StartupStage, StartupTiming};

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

use imp_core::imp_session::{
    resolve_runtime_connection, ImpSession, ResolvedRuntimeConnection, RuntimeConnectionIntent,
    SessionChoice, SessionOptions,
};
use imp_core::runtime::RuntimeStateAccumulator;
use imp_core::ui::{ComponentSpec, NotifyLevel, SelectOption, UserInterface, WidgetContent};
use imp_core::usage::{UsageCostBreakdown, UsageRecordSource, UsageTokens};
use imp_core::TimingEvent;
use imp_llm::auth::AuthStore;
use imp_llm::model::ModelRegistry;
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
mod view;

/// A coding agent engine
#[derive(Parser)]
#[command(name = "imp", version, about)]
pub struct Cli {
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

    /// Final output format for --print: text, json, or jsonl
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
    /// Open the session viewer/inspector surface
    View {
        /// Viewer area to open: sessions, tree, logs, checkpoints
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
        command: Option<secrets::SecretsCommand>,
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
    /// Repeat the same prompt until an exit condition is met
    Loop(LoopArgs),
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

#[derive(Args, Debug, Clone)]
struct LoopArgs {
    /// Maximum number of prompt repetitions
    #[arg(long)]
    steps: Option<u32>,

    /// Stop when the model output contains this text
    #[arg(long)]
    until: Option<String>,

    /// Stop when this shell command exits successfully after an iteration
    #[arg(long)]
    done: Option<String>,

    /// Prompt to repeat. @file arguments include file content as in one-shot mode.
    #[arg(trailing_var_arg = true, required = true)]
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum EvidenceCommand {
    /// List recent run evidence records
    List,
    /// Print the latest evidence HTML path
    Latest,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliRunDisposition {
    Headless,
    Tui,
}

impl Cli {
    pub fn no_session(&self) -> bool {
        self.no_session
    }

    pub fn continue_recent(&self) -> bool {
        self.cont
    }

    pub fn session_path(&self) -> Option<&Path> {
        self.session.as_deref()
    }

    pub fn model_override(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn thinking_override(&self) -> Option<&str> {
        self.thinking.as_deref()
    }
}

pub fn parse_cli() -> Cli {
    Cli::parse()
}

pub fn disposition(cli: &Cli) -> CliRunDisposition {
    match cli.command.as_ref() {
        Some(Commands::Chat | Commands::Tui) => CliRunDisposition::Tui,
        _ if cli.command.is_some() => CliRunDisposition::Headless,
        _ if cli.list_models => CliRunDisposition::Headless,
        _ if cli.mode == "interactive"
            && cli.print.is_none()
            && prompt_args(&cli.args).is_empty() =>
        {
            CliRunDisposition::Tui
        }
        _ => CliRunDisposition::Headless,
    }
}

pub async fn run() {
    let cli = parse_cli();
    if disposition(&cli) == CliRunDisposition::Tui {
        eprintln!("Error: TUI mode is provided by the imp binary composition crate.");
        std::process::exit(1);
    }
    run_headless(cli).await;
}

pub async fn run_headless(cli: Cli) {
    // Dispatch subcommands first
    if let Some(command) = &cli.command {
        match command {
            Commands::Chat => {
                eprintln!("Error: TUI mode is provided by the imp binary composition crate.");
                std::process::exit(1);
            }
            Commands::Acp => {
                if let Err(e) = acp::run_stdio_server(env!("CARGO_PKG_VERSION")).await {
                    eprintln!("ACP server failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Tui => {
                eprintln!("Error: TUI mode is provided by the imp binary composition crate.");
                std::process::exit(1);
            }
            Commands::View { area } => {
                if let Err(e) = view::run(area.as_deref()).await {
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
                if let Err(e) = setup::run_setup_mode().await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Login { provider } => {
                if let Err(e) = setup::run_login_command(provider.as_deref()).await {
                    eprintln!("Login failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Secrets { command, provider } => {
                if let Err(e) = secrets::run_command(command.as_ref(), provider.as_deref()).await {
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
            Commands::Loop(args) => {
                if let Err(e) = run_loop_mode(&cli, args).await {
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
                import::run(*dry_run, from.as_deref(), *yes);
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
                if let Err(e) = setup::run_web_login(provider).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
        }
    }

    // List models
    if cli.list_models {
        models::run_list();
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

    // Default interactive mode is owned by imp-bin, not imp-cli.
    if cli.mode == "interactive" {
        eprintln!("Error: TUI mode is provided by the imp binary composition crate.");
        std::process::exit(1);
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

pub fn parse_thinking_level(s: &str) -> ThinkingLevel {
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
            "raw_total_tokens": usage.raw_total_tokens(),
            "effective_total_tokens": usage.effective_total_tokens(),
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
        AgentEvent::ContextUsageUpdated {
            used,
            display_window,
            input_limit,
            system_tokens,
            tool_definition_tokens,
            message_tokens,
            output_tokens,
            observed_input_limit,
        } => json!({
            "type": "context_usage_updated",
            "used": used,
            "display_window": display_window,
            "input_limit": input_limit,
            "system_tokens": system_tokens,
            "tool_definition_tokens": tool_definition_tokens,
            "message_tokens": message_tokens,
            "output_tokens": output_tokens,
            "observed_input_limit": observed_input_limit,
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
    Jsonl,
}

impl PrintOutputMode {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" | "human" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "jsonl" | "benchmark-jsonl" => Ok(Self::Jsonl),
            other => Err(format!(
                "unknown --output mode `{other}`; use text, json, or jsonl"
            )),
        }
    }

    fn is_structured(self) -> bool {
        matches!(self, Self::Json | Self::Jsonl)
    }
}

#[derive(Debug, Default, Serialize)]
struct PrintJsonOutcome {
    status: String,
    final_text: String,
    policy_violations: Vec<PrintPolicyViolation>,
    tool_calls: Vec<PrintToolCall>,
    metrics: PrintRunMetrics,
    usage: Option<PrintUsage>,
    cost: Option<PrintCost>,
}

#[derive(Debug, Default, Serialize)]
struct PrintRunMetrics {
    wall_time_ms: Option<u64>,
    ttft_ms: Option<u64>,
    first_stream_event_ms: Option<u64>,
    turns: u32,
    tool_calls: u32,
    failed_tool_calls: u32,
    files_read: u32,
    files_written: u32,
    commands_run: u32,
    verification: PrintVerificationSummary,
}

#[derive(Debug, Default, Serialize)]
struct PrintVerificationSummary {
    commands: Vec<String>,
    passed: Option<bool>,
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
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    raw_total_tokens: u32,
    effective_total_tokens: u32,
}

#[derive(Debug, Serialize)]
struct PrintCost {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    total: f64,
}

fn print_usage(usage: &imp_llm::Usage) -> PrintUsage {
    PrintUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        raw_total_tokens: usage.raw_total_tokens(),
        effective_total_tokens: usage.effective_total_tokens(),
    }
}

fn print_cost(cost: &imp_llm::Cost) -> PrintCost {
    PrintCost {
        input: cost.input,
        output: cost.output,
        cache_read: cost.cache_read,
        cache_write: cost.cache_write,
        total: cost.total,
    }
}

fn emit_print_jsonl_event(value: Value) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string(&value)?);
    io::stdout().flush()?;
    Ok(())
}

fn final_summary_jsonl(outcome: &PrintJsonOutcome) -> Value {
    let mut value = serde_json::to_value(outcome).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("type".into(), Value::String("final_summary".into()));
    }
    value
}

fn update_verification_summary(metrics: &mut PrintRunMetrics, gate: &VerificationGate) {
    if let Some(command) = &gate.command {
        if !metrics
            .verification
            .commands
            .iter()
            .any(|existing| existing == &command.command)
        {
            metrics.verification.commands.push(command.command.clone());
        }
    }

    match gate.status {
        imp_core::workflow::VerificationGateStatus::Failed
        | imp_core::workflow::VerificationGateStatus::Blocked => {
            metrics.verification.passed = Some(false);
        }
        imp_core::workflow::VerificationGateStatus::Passed => {
            if metrics.verification.passed.is_none() {
                metrics.verification.passed = Some(true);
            }
        }
        imp_core::workflow::VerificationGateStatus::Pending
        | imp_core::workflow::VerificationGateStatus::Running
        | imp_core::workflow::VerificationGateStatus::Skipped => {}
    }
}

async fn run_loop_mode(cli: &Cli, args: &LoopArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.until.is_none() && args.done.is_none() && args.steps.is_none() {
        return Err(
            "loop requires at least one exit condition: --steps, --until, or --done".into(),
        );
    }

    let max_steps = args.steps.unwrap_or(u32::MAX);
    if max_steps == 0 {
        return Ok(());
    }

    let file_context = expand_file_args(&args.prompt);
    let prompt = prompt_args(&args.prompt).join(" ");
    if prompt.trim().is_empty() && file_context.trim().is_empty() {
        return Err("loop requires a non-empty prompt".into());
    }
    let full_prompt = build_full_prompt(&prompt, &file_context, &None);

    for step in 1..=max_steps {
        eprintln!("[loop: step {step}]");
        let outcome = run_print_mode(cli, &full_prompt).await?;

        if let Some(until) = &args.until {
            if outcome.final_text.contains(until) {
                eprintln!("[loop: stopped because output matched --until]");
                break;
            }
        }

        if let Some(done) = &args.done {
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(done)
                .status()?;
            if status.success() {
                eprintln!("[loop: stopped because --done command succeeded]");
                break;
            }
        }
    }

    Ok(())
}

async fn run_print_mode(
    cli: &Cli,
    prompt: &str,
) -> Result<PrintJsonOutcome, Box<dyn std::error::Error>> {
    let run_started_at = std::time::Instant::now();
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
    let structured_output = print_output_mode.is_structured();
    let json_output = print_output_mode == PrintOutputMode::Json;
    let jsonl_output = print_output_mode == PrintOutputMode::Jsonl;
    let mut json_outcome = PrintJsonOutcome {
        status: "done".to_string(),
        ..Default::default()
    };
    let mut active_tool: Option<String> = None;

    while let Some(event) = session.recv_event().await {
        if jsonl_output {
            emit_print_jsonl_event(rpc_agent_event_legacy_json(&event))?;
        }
        match event {
            AgentEvent::MessageDelta { delta } => match delta {
                StreamEvent::TextDelta { text } => {
                    json_outcome.final_text.push_str(&text);
                    if structured_output {
                        if json_outcome.metrics.ttft_ms.is_none() {
                            json_outcome.metrics.ttft_ms =
                                Some(run_started_at.elapsed().as_millis() as u64);
                        }
                    } else {
                        print!("{text}");
                        printed_trailing_newline = false;
                    }
                }
                StreamEvent::ThinkingDelta { text } => {
                    if !structured_output {
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
                if !structured_output {
                    if summary.is_empty() {
                        eprintln!("[tool: {tool_name}]");
                    } else {
                        eprintln!("[tool: {tool_name} {summary}]");
                    }
                }
                json_outcome.metrics.tool_calls += 1;
                match tool_name.as_str() {
                    "bash" => json_outcome.metrics.commands_run += 1,
                    "read" => json_outcome.metrics.files_read += 1,
                    "write" | "edit" => json_outcome.metrics.files_written += 1,
                    _ => {}
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
                if structured_output {
                    if result.is_error {
                        json_outcome.metrics.failed_tool_calls += 1;
                    }
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
            AgentEvent::TurnStart { index } => {
                json_outcome.metrics.turns = json_outcome.metrics.turns.max(index + 1);
            }
            AgentEvent::TurnEnd { .. } => {
                if !structured_output && !printed_trailing_newline {
                    println!();
                    printed_trailing_newline = true;
                }
            }
            AgentEvent::Error { error } => {
                json_outcome.status = "failed".to_string();
                if structured_output {
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
                match timing.stage {
                    imp_core::TimingStage::FirstStreamEvent => {
                        if json_outcome.metrics.first_stream_event_ms.is_none() {
                            json_outcome.metrics.first_stream_event_ms =
                                Some(run_started_at.elapsed().as_millis() as u64);
                        }
                    }
                    imp_core::TimingStage::FirstTextDelta => {
                        if json_outcome.metrics.ttft_ms.is_none() {
                            json_outcome.metrics.ttft_ms =
                                Some(run_started_at.elapsed().as_millis() as u64);
                        }
                    }
                    _ => {}
                }
                if cli.verbose && !structured_output {
                    eprintln!("{}", format_timing_event(&timing));
                }
            }
            AgentEvent::AgentEnd {
                usage,
                cost,
                status,
            } => {
                if json_outcome.status == "done" {
                    json_outcome.status = match status {
                        imp_core::agent::RunFinalStatus::Done { .. } => "done".to_string(),
                        imp_core::agent::RunFinalStatus::DoneWithConcerns { .. } => {
                            "done_with_concerns".to_string()
                        }
                        imp_core::agent::RunFinalStatus::Blocked { .. } => "blocked".to_string(),
                        imp_core::agent::RunFinalStatus::NeedsUserInput { .. } => {
                            "needs_user_input".to_string()
                        }
                        imp_core::agent::RunFinalStatus::Cancelled => "cancelled".to_string(),
                        imp_core::agent::RunFinalStatus::Failed { .. } => "failed".to_string(),
                    };
                }
                if structured_output {
                    json_outcome.usage = Some(print_usage(&usage));
                    json_outcome.cost = Some(print_cost(&cost));
                } else {
                    eprintln!(
                        "\n[tokens: raw={} effective={} (↑{} ↓{} cache_read={} cache_write={}) | cost: ${:.4}]",
                        usage.raw_total_tokens(),
                        usage.effective_total_tokens(),
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_write_tokens,
                        cost.total
                    );
                }
            }
            AgentEvent::VerificationCompleted { gate, .. } => {
                update_verification_summary(&mut json_outcome.metrics, &gate);
            }
            _ => {}
        }
    }

    session
        .wait()
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    json_outcome.metrics.wall_time_ms = Some(run_started_at.elapsed().as_millis() as u64);

    if json_output {
        println!("{}", serde_json::to_string(&json_outcome)?);
    } else if jsonl_output {
        emit_print_jsonl_event(final_summary_jsonl(&json_outcome))?;
    }

    Ok(json_outcome)
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

pub(crate) fn web_search_provider_label(provider: Option<SearchProvider>) -> &'static str {
    match provider {
        Some(provider) => provider.name(),
        None => "none",
    }
}

pub(crate) fn prompt_input_line(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
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

pub(crate) fn save_user_config(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
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

pub(crate) fn thinking_level_label(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
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
#[path = "lib/tests.rs"]
mod tests;
