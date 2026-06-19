use std::io::{self, Write};

use imp_core::agent::AgentEvent;
use imp_core::config::Config;
use imp_core::format_error_for_display;
use imp_core::imp_session::{ImpSession, SessionChoice, SessionOptions};
use imp_core::workflow::VerificationGate;
use imp_llm::{truncate_chars_with_suffix, StreamEvent};
use serde::Serialize;
use serde_json::Value;

use crate::prompt::{build_full_prompt, expand_file_args, prompt_args};
use crate::rpc::rpc_agent_event_legacy_json;
use crate::startup_timing::{emit_startup_timing, StartupStage, StartupTimer};
use crate::{
    build_lua_loader, cli_verification_gates, format_timing_event, parse_thinking_level, Cli,
    LoopArgs,
};

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
pub(crate) struct PrintJsonOutcome {
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

pub(crate) async fn run_loop_mode(
    cli: &Cli,
    args: &LoopArgs,
) -> Result<(), Box<dyn std::error::Error>> {
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

pub(crate) async fn run_print_mode(
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
