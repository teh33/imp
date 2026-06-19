use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod cli_config;
mod evidence;
mod import;
mod local_install;
mod models;
mod print_mode;
mod prompt;
mod provider_secrets;
mod reporting;
mod rpc;
mod secrets;
mod settings;
mod setup;
mod startup_timing;
use local_install::run_install_local;
use prompt::{build_full_prompt, expand_file_args, prompt_args};
pub use startup_timing::{StartupStage, StartupTiming};

use clap::{Args, Parser, Subcommand};
#[cfg(test)]
use imp_core::agent::AgentEvent;
use imp_core::config::Config;
use imp_core::workflow::{AutonomyMode, VerificationGate};

use imp_core::imp_session::{
    resolve_runtime_connection, ResolvedRuntimeConnection, RuntimeConnectionIntent,
};
#[cfg(test)]
use imp_core::runtime::RuntimeStateAccumulator;
use imp_core::TimingEvent;
use imp_llm::auth::AuthStore;
use imp_llm::model::ModelRegistry;
use imp_llm::provider::ThinkingLevel;

pub(crate) mod acp;
mod stats_report;
mod usage_report;
mod view;
mod workflow_cli;

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
        command: stats_report::StatsCommand,
    },
    /// Usage reporting and export
    Usage {
        #[command(subcommand)]
        command: usage_report::UsageCommand,
    },
    /// Inspect, validate, run, and update native workflow artifacts
    Workflow {
        #[command(subcommand)]
        command: workflow_cli::WorkflowCommand,
    },
    /// Repeat the same prompt until an exit condition is met
    Loop(LoopArgs),
    /// Open or inspect run evidence artifacts
    Evidence {
        #[command(subcommand)]
        command: Option<evidence::EvidenceCommand>,
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

#[derive(Args, Debug, Clone)]
pub(crate) struct LoopArgs {
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
                if let Err(e) = settings::run() {
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
                if let Err(e) = workflow_cli::run_command(command).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Loop(args) => {
                if let Err(e) = print_mode::run_loop_mode(&cli, args).await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Commands::Evidence { command } => {
                if let Err(e) = evidence::run(command.as_ref()) {
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
        if let Err(e) = print_mode::run_print_mode(&cli, &full_prompt).await {
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
        if let Err(e) = print_mode::run_print_mode(&cli, &full_prompt).await {
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
        if let Err(e) = print_mode::run_print_mode(&cli, &full_prompt).await {
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
            if let Err(e) = rpc::run(&cli).await {
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

pub(crate) fn parse_thinking_level(s: &str) -> ThinkingLevel {
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

pub(crate) fn resolve_model_and_provider(
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

pub(crate) async fn resolve_provider_api_key(
    auth_store: &mut AuthStore,
    provider_name: &str,
) -> Result<imp_llm::auth::ApiKey, imp_llm::Error> {
    match provider_name {
        "openai-codex" => auth_store.resolve_chatgpt_oauth().await,
        "anthropic" | "kimi-code" => auth_store.resolve_with_refresh(provider_name).await,
        _ => auth_store.resolve(provider_name),
    }
}

pub(crate) fn build_lua_loader(
    no_tools: bool,
    cwd: PathBuf,
) -> Option<imp_core::tools::LuaToolLoader> {
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

pub(crate) fn format_timing_event(timing: &TimingEvent) -> String {
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

pub(crate) fn cli_verification_gates(commands: &[String]) -> Vec<VerificationGate> {
    commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            VerificationGate::command(format!("cli-verify-{}", index + 1), command.clone())
        })
        .collect()
}

#[cfg(test)]
fn rpc_agent_event_to_json(event: &AgentEvent) -> Value {
    let runtime_event = event.to_runtime_event("rpc", 0);
    let mut runtime_state = RuntimeStateAccumulator::new("rpc");
    runtime_state.apply(&runtime_event);
    rpc_agent_event_to_json_with_runtime(event, &runtime_event, &runtime_state.snapshot())
}

#[cfg(test)]
#[path = "lib/tests.rs"]
mod tests;
