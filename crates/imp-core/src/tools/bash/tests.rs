use super::*;
use crate::config::{AllowedCommandSecret, CommandSecretsConfig, Config, SecretsConfig};
use crate::ui::NullInterface;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// Tests use sh for deterministic behavior (rush has exit code bugs: rush#8)
fn ensure_sh() {
    std::env::set_var("IMP_SHELL", "sh");
}

fn test_ctx(dir: &std::path::Path) -> (ToolContext, tokio::sync::mpsc::Receiver<ToolUpdate>) {
    ensure_sh();
    let (tx, rx) = tokio::sync::mpsc::channel(1024);
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
    let ctx = ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(AtomicBool::new(false)),
        update_tx: tx,
        command_tx: cmd_tx,
        ui: Arc::new(NullInterface),
        file_cache: Arc::new(crate::tools::FileCache::new()),
        checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
        anchor_store: Arc::new(crate::tools::AnchorStore::new()),
        lua_tool_loader: None,
        mode: crate::config::AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(crate::config::Config::default()),
        run_policy: Default::default(),
        supporting_provenance: Vec::new(),
    };
    (ctx, rx)
}

fn allow_test_secret(ctx: &mut ToolContext) {
    ctx.config = Arc::new(Config {
        secrets: SecretsConfig {
            commands: CommandSecretsConfig {
                enabled: true,
                allowed: vec![AllowedCommandSecret {
                    name: "test-service".to_string(),
                }],
            },
        },
        ..Config::default()
    });
}

#[tokio::test]
async fn with_secrets_injects_allowed_secret_and_redacts_output() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("TEST_SERVICE_API_KEY", "native-secret-value");
    let (mut ctx, _rx) = test_ctx(tmp.path());
    allow_test_secret(&mut ctx);

    let result = run_command(
        "printf '%s' \"$TEST_SERVICE_API_KEY\"",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        vec![RequestedSecret {
            name: "test-service".to_string(),
        }],
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    let text = result.text_content().unwrap();
    assert!(text.contains(SECRET_REDACTION));
    assert!(!text.contains("native-secret-value"));
    assert_eq!(
        result.details["with_secrets"][0].as_str(),
        Some("test-service")
    );
    assert_eq!(
        result.details["injected_env"][0].as_str(),
        Some("TEST_SERVICE_API_KEY")
    );
    assert!(!result.details.to_string().contains("native-secret-value"));
}

#[tokio::test]
async fn with_secrets_redacts_truncation_temp_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("TEST_SERVICE_API_KEY", "native-secret-value");
    let (mut ctx, mut rx) = test_ctx(tmp.path());
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    allow_test_secret(&mut ctx);

    let result = run_command(
        "for i in $(seq 1 2105); do printf '%s line-%s\\n' \"$TEST_SERVICE_API_KEY\" \"$i\"; done",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        vec![RequestedSecret {
            name: "test-service".to_string(),
        }],
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    assert!(result.details["truncated"].as_bool().unwrap_or(false));
    let text = result.text_content().unwrap();
    assert!(!text.contains("native-secret-value"));
    let temp_path = text
        .split("Full output saved to ")
        .nth(1)
        .and_then(|tail| tail.lines().next())
        .expect("truncation note should include temp file path")
        .trim_end_matches(']');
    let temp_content = std::fs::read_to_string(temp_path).unwrap();
    assert!(temp_content.contains(SECRET_REDACTION));
    assert!(!temp_content.contains("native-secret-value"));
    drop(result);
    drop(ctx);
    drain.abort();
}

#[tokio::test]
async fn with_secrets_denies_unconfigured_secret() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("TEST_SERVICE_API_KEY", "native-secret-value");
    let (ctx, _rx) = test_ctx(tmp.path());

    let err = match run_command(
        "true",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        vec![RequestedSecret {
            name: "test-service".to_string(),
        }],
    )
    .await
    {
        Ok(_) => panic!("expected policy denial"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(message.contains("not allowed by config policy"));
    assert!(!message.contains("native-secret-value"));
}

#[tokio::test]
async fn with_secrets_rejects_duplicate_secret_names() {
    let err =
        parse_with_secrets(Some(&serde_json::json!(["test-service", "test-service"]))).unwrap_err();

    assert!(err.to_string().contains("duplicate with_secrets entry"));
}

#[tokio::test]
async fn with_secrets_rejects_non_string_entries() {
    let err = parse_with_secrets(Some(&serde_json::json!([
        {"provider":"one", "field":"api_key"}
    ])))
    .unwrap_err();

    assert!(err.to_string().contains("non-empty strings"));
}

#[test]
fn field_env_name_is_deterministic() {
    assert_eq!(
        field_env_name("openrouter", "api_key"),
        "OPENROUTER_API_KEY"
    );
    assert_eq!(
        field_env_name("porkbun", "secrets_key"),
        "PORKBUN_SECRET_KEY"
    );
}

#[tokio::test]
async fn bash_simple_command() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command("echo hello world", DEFAULT_TIMEOUT_SECS, &ctx, Vec::new())
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("hello world"));
    assert_eq!(result.details["exit_code"], 0);
}

#[tokio::test]
async fn bash_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command("exit 42", DEFAULT_TIMEOUT_SECS, &ctx, Vec::new())
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(result.details["exit_code"], 42);
}

#[tokio::test]
async fn bash_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command("sleep 60", 1, &ctx, Vec::new()).await.unwrap();

    assert!(result.details["timed_out"].as_bool().unwrap());
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("timed out"));
}

#[tokio::test]
async fn bash_cancellation() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    // Set cancelled before running — should return immediately.
    ctx.cancelled
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let result = run_command("sleep 60", DEFAULT_TIMEOUT_SECS, &ctx, Vec::new())
        .await
        .unwrap();

    assert!(result.details["cancelled"].as_bool().unwrap());
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("cancelled"));
}

#[tokio::test]
async fn bash_cancellation_during_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());
    let cancelled = Arc::clone(&ctx.cancelled);

    let task = tokio::spawn(async move {
        run_command("sleep 60", DEFAULT_TIMEOUT_SECS, &ctx, Vec::new()).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);

    let result = task.await.unwrap().unwrap();
    assert!(result.details["cancelled"].as_bool().unwrap());
}

#[tokio::test]
async fn bash_streaming_output() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, mut rx) = test_ctx(tmp.path());

    let handle = tokio::spawn(async move {
        run_command(
            "echo line1; echo line2; echo line3",
            DEFAULT_TIMEOUT_SECS,
            &ctx,
            Vec::new(),
        )
        .await
    });

    // Collect streamed updates
    let mut updates = Vec::new();
    while let Some(update) = rx.recv().await {
        updates.push(update);
    }

    let result = handle.await.unwrap().unwrap();
    assert!(!result.is_error);
    assert!(
        !updates.is_empty(),
        "should have received streaming updates"
    );
}

#[tokio::test]
async fn bash_stdout_and_stderr_merged() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command(
        "echo stdout_line; echo stderr_line >&2",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        Vec::new(),
    )
    .await
    .unwrap();

    // exit code 0 → not an error
    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("stdout_line"));
    assert!(text.contains("stderr_line"));
}

#[tokio::test]
async fn bash_writes_file_side_effect() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command(
        "echo 'side effect content' > side_effect.txt",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        Vec::new(),
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    let written = std::fs::read_to_string(tmp.path().join("side_effect.txt")).unwrap();
    assert!(written.contains("side effect content"));
}

#[tokio::test]
async fn bash_uses_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("testfile.txt"), "content").unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command("ls testfile.txt", DEFAULT_TIMEOUT_SECS, &ctx, Vec::new())
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("testfile.txt"));
}

#[tokio::test]
async fn bash_strips_ansi_sequences() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command(
        "printf '\\033[1;31mred\\033[0m\\n'",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        Vec::new(),
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("red"));
    assert!(!text.contains("\u{1b}[1;31m"));
    assert!(!text.contains("\u{1b}[0m"));
}

#[tokio::test]
async fn bash_workdir_override_executes_in_target_dir() {
    let root = tempfile::tempdir().unwrap();
    let subdir = root.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    std::fs::write(subdir.join("inside.txt"), "ok").unwrap();
    let tool = BashTool;
    let (ctx, _rx) = test_ctx(root.path());

    let result = tool
        .execute(
            "c-workdir",
            serde_json::json!({"command": "ls inside.txt", "workdir": "subdir"}),
            ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("inside.txt"));
}

#[tokio::test]
async fn bash_invalid_workdir_returns_error() {
    let root = tempfile::tempdir().unwrap();
    let tool = BashTool;
    let (ctx, _rx) = test_ctx(root.path());

    let result = tool
        .execute(
            "c-bad-workdir",
            serde_json::json!({"command": "pwd", "workdir": "missing-dir"}),
            ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    let text = match &result.content[0] {
        imp_llm::ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("workdir not found"));
}

#[tokio::test]
async fn bash_treats_rg_no_matches_as_success() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("afile.txt"), "haystack\n").unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command(
        "rg definitely_not_present .",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        Vec::new(),
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.details["exit_code"], 1);
    assert!(result.text_content().unwrap().contains("No matches found"));
}

#[tokio::test]
async fn bash_command_not_found_returns_actionable_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _rx) = test_ctx(tmp.path());

    let result = run_command(
        "definitely_not_a_real_command_98765",
        DEFAULT_TIMEOUT_SECS,
        &ctx,
        Vec::new(),
    )
    .await
    .unwrap();

    assert!(result.is_error);
    assert_eq!(result.details["exit_code"], 127);
    assert!(result.text_content().unwrap().contains("Command not found"));
}

// ── rush backend tests ──────────────────────────────────────────
//
// Call run_via_rush directly to avoid env-var races between
// parallel test threads.

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_echo() {
    let tmp = tempfile::tempdir().unwrap();
    let (output, exit_code, timed_out, _truncated) =
        run_via_rush("echo hello world", DEFAULT_TIMEOUT_SECS, tmp.path(), false)
            .expect("rush should succeed");

    assert_eq!(exit_code, 0);
    assert!(!timed_out);
    assert!(output.contains("hello world"), "stdout missing: {output}");
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("afile.txt"), "content").unwrap();

    let (output, exit_code, _, _) =
        run_via_rush("ls", DEFAULT_TIMEOUT_SECS, tmp.path(), false).expect("rush should succeed");

    assert_eq!(exit_code, 0);
    assert!(
        output.contains("afile.txt"),
        "ls should list file: {output}"
    );
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_ls_json_text_transform() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("afile.txt"), "content").unwrap();

    let (output, exit_code, _, _) =
        run_via_rush("ls", DEFAULT_TIMEOUT_SECS, tmp.path(), true).expect("rush should succeed");
    let text = parse_json_lines_to_text("ls", &output).expect("json should transform");

    assert_eq!(exit_code, 0);
    assert!(text.contains("afile.txt"));
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_grep_json_text_transform() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("afile.txt"), "hello needle world\n").unwrap();

    let (output, exit_code, _, _) =
        run_via_rush("grep -r needle .", DEFAULT_TIMEOUT_SECS, tmp.path(), true)
            .expect("rush should succeed");
    let text =
        parse_json_lines_to_text("grep -r needle .", &output).expect("json should transform");

    assert_eq!(exit_code, 0);
    assert!(text.contains("needle"));
    assert!(
        text.contains("afile.txt") || text.contains(":1:"),
        "unexpected grep text: {text}"
    );
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_find_json_text_transform() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("afile.txt"), "content").unwrap();

    let (output, exit_code, _, _) = run_via_rush(
        "find . -name afile.txt",
        DEFAULT_TIMEOUT_SECS,
        tmp.path(),
        true,
    )
    .expect("rush should succeed");
    let text =
        parse_json_lines_to_text("find . -name afile.txt", &output).expect("json should transform");

    assert_eq!(exit_code, 0);
    assert!(text.contains("afile.txt"));
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_pipeline() {
    let tmp = tempfile::tempdir().unwrap();

    let (output, exit_code, _, _) =
        run_via_rush("echo foo | cat", DEFAULT_TIMEOUT_SECS, tmp.path(), false)
            .expect("rush should succeed");

    assert_eq!(exit_code, 0);
    assert!(output.contains("foo"), "pipeline output missing: {output}");
}

#[test]
#[cfg(feature = "rush-backend")]
fn test_rush_backend_exit_code() {
    let tmp = tempfile::tempdir().unwrap();

    let (_, exit_code, _, _) = run_via_rush("exit 42", DEFAULT_TIMEOUT_SECS, tmp.path(), false)
        .expect("rush should return result even on non-zero exit");

    assert_eq!(exit_code, 42);
}
