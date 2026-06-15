use super::*;
use crate::local_install::resolve_install_destination;
use imp_llm::auth::{OAuthCredential, StoredCredential};
use imp_llm::stream::StreamEvent;
use serde_json::json;
use std::ffi::OsString;

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
fn cli_parses_loop_command() {
    let cli = Cli::try_parse_from([
        "imp",
        "loop",
        "--steps",
        "10",
        "--until",
        "DONE",
        "--done",
        "cargo test",
        "fix",
        "it",
    ])
    .expect("parse loop command");

    match cli.command {
        Some(Commands::Loop(args)) => {
            assert_eq!(args.steps, Some(10));
            assert_eq!(args.until.as_deref(), Some("DONE"));
            assert_eq!(args.done.as_deref(), Some("cargo test"));
            assert_eq!(args.prompt, vec!["fix".to_string(), "it".to_string()]);
        }
        other => panic!("expected loop command, got {other:?}"),
    }
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
fn cli_disposition_routes_tui_modes_to_composition_crate() {
    let default_cli = Cli::try_parse_from(["imp"]).expect("parse default cli");
    assert_eq!(disposition(&default_cli), CliRunDisposition::Tui);

    let tui_cli = Cli::try_parse_from(["imp", "tui"]).expect("parse tui command");
    assert_eq!(disposition(&tui_cli), CliRunDisposition::Tui);

    let chat_cli = Cli::try_parse_from(["imp", "chat"]).expect("parse chat command");
    assert_eq!(disposition(&chat_cli), CliRunDisposition::Tui);
}

#[test]
fn cli_disposition_routes_headless_modes_to_imp_cli() {
    let print_cli = Cli::try_parse_from(["imp", "-p", "say ready"]).expect("parse print command");
    assert_eq!(disposition(&print_cli), CliRunDisposition::Headless);

    let rpc_cli = Cli::try_parse_from(["imp", "--mode", "rpc"]).expect("parse rpc mode");
    assert_eq!(disposition(&rpc_cli), CliRunDisposition::Headless);

    let workflow_cli =
        Cli::try_parse_from(["imp", "workflow", "list"]).expect("parse workflow command");
    assert_eq!(disposition(&workflow_cli), CliRunDisposition::Headless);

    let bare_prompt_cli = Cli::try_parse_from(["imp", "fix", "this"]).expect("parse bare prompt");
    assert_eq!(disposition(&bare_prompt_cli), CliRunDisposition::Headless);
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
    let (model_id, _) = resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
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
    let (_, provider) = resolve_model_and_provider(&cli, &config, &registry, &auth_store).unwrap();
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
    assert_eq!(json["cache_write_tokens"], 50);
    assert_eq!(json["raw_total_tokens"], 1500);
    assert_eq!(json["effective_total_tokens"], 1400);
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
fn print_usage_includes_cache_aware_totals() {
    let usage = imp_llm::Usage {
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 250,
        cache_write_tokens: 75,
    };

    let json = serde_json::to_value(print_usage(&usage)).unwrap();
    assert_eq!(json["input_tokens"], 1000);
    assert_eq!(json["output_tokens"], 500);
    assert_eq!(json["cache_read_tokens"], 250);
    assert_eq!(json["cache_write_tokens"], 75);
    assert_eq!(json["raw_total_tokens"], 1500);
    assert_eq!(json["effective_total_tokens"], 1250);
}

#[test]
fn final_summary_jsonl_tags_summary_without_losing_metrics() {
    let mut outcome = PrintJsonOutcome {
        status: "done".to_string(),
        final_text: "ok".to_string(),
        ..PrintJsonOutcome::default()
    };
    outcome.metrics.wall_time_ms = Some(42);
    outcome.metrics.tool_calls = 2;
    outcome.usage = Some(print_usage(&imp_llm::Usage {
        input_tokens: 10,
        output_tokens: 5,
        cache_read_tokens: 4,
        cache_write_tokens: 0,
    }));

    let json = final_summary_jsonl(&outcome);
    assert_eq!(json["type"], "final_summary");
    assert_eq!(json["status"], "done");
    assert_eq!(json["final_text"], "ok");
    assert_eq!(json["metrics"]["wall_time_ms"], 42);
    assert_eq!(json["metrics"]["tool_calls"], 2);
    assert_eq!(json["usage"]["effective_total_tokens"], 11);
}

#[test]
fn update_verification_summary_dedupes_commands_and_tracks_failures() {
    let mut metrics = PrintRunMetrics::default();
    let mut passed_gate = VerificationGate::command("tests", "cargo test -p imp-cli");
    passed_gate.status = imp_core::workflow::VerificationGateStatus::Passed;
    update_verification_summary(&mut metrics, &passed_gate);
    update_verification_summary(&mut metrics, &passed_gate);
    assert_eq!(metrics.verification.commands, vec!["cargo test -p imp-cli"]);
    assert_eq!(metrics.verification.passed, Some(true));

    let mut failed_gate = VerificationGate::command("fmt", "cargo fmt --check");
    failed_gate.status = imp_core::workflow::VerificationGateStatus::Failed;
    update_verification_summary(&mut metrics, &failed_gate);
    assert_eq!(
        metrics.verification.commands,
        vec!["cargo test -p imp-cli", "cargo fmt --check"]
    );
    assert_eq!(metrics.verification.passed, Some(false));
}

#[test]
fn startup_stage_names_are_stable() {
    assert_eq!(StartupStage::ProcessStart.as_str(), "process_start");
    assert_eq!(StartupStage::RunLoopStarted.as_str(), "run_loop_started");
}
