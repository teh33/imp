use std::process::Stdio;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use imp_llm::auth::AuthStore;

use super::{
    truncate_head, truncate_tail, Tool, ToolContext, ToolOutput, ToolUpdate, TruncationResult,
};
use crate::error::{Error, Result};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_LINES: usize = 2000;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

const SECRET_REDACTION: &str = "[REDACTED_SECRET]";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestedSecret {
    name: String,
}

struct ResolvedSecretEnvBinding {
    secret_name: String,
    env: String,
    value: String,
}

fn parse_with_secrets(value: Option<&Value>) -> Result<Vec<RequestedSecret>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let entries = value
        .as_array()
        .ok_or_else(|| Error::Tool("with_secrets must be an array".into()))?;
    let mut requested = Vec::with_capacity(entries.len());
    let mut names = std::collections::HashSet::new();

    for entry in entries {
        let name = entry
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error::Tool("with_secrets entries must be non-empty strings".into()))?;
        if !names.insert(name.to_string()) {
            return Err(Error::Tool(format!("duplicate with_secrets entry: {name}")));
        }
        requested.push(RequestedSecret {
            name: name.to_string(),
        });
    }

    Ok(requested)
}

fn secret_name_allowed(ctx: &ToolContext, name: &str) -> bool {
    let policy = &ctx.config.secrets.commands;
    policy.enabled && policy.allowed.iter().any(|allowed| allowed.name == name)
}

fn field_env_name(secret_name: &str, field: &str) -> String {
    let canonical_field = if field == "secrets_key" {
        "secret_key"
    } else {
        field
    };
    format!("{secret_name}_{canonical_field}")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn resolve_with_secrets(
    ctx: &ToolContext,
    requested: Vec<RequestedSecret>,
) -> Result<Vec<ResolvedSecretEnvBinding>> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    for secret in &requested {
        if !secret_name_allowed(ctx, &secret.name) {
            return Err(Error::Tool(format!(
                "with_secrets entry {} is not allowed by config policy",
                secret.name
            )));
        }
    }

    let auth_path = crate::storage::existing_global_auth_path()
        .unwrap_or_else(crate::storage::global_auth_path);
    let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
    let mut resolved = Vec::new();
    let mut env_names = std::collections::HashSet::new();

    for secret in requested {
        let fields = auth_store
            .resolve_secret_fields(&secret.name)
            .map_err(|error| Error::Tool(format!("missing secret for {}: {error}", secret.name)))?;
        for (field, value) in fields {
            let env = field_env_name(&secret.name, &field);
            if !is_valid_env_name(&env) {
                return Err(Error::Tool(format!(
                    "derived env name '{env}' for {}.{field} is invalid",
                    secret.name
                )));
            }
            if !env_names.insert(env.clone()) {
                return Err(Error::Tool(format!(
                    "duplicate derived env var {env} from with_secrets"
                )));
            }
            resolved.push(ResolvedSecretEnvBinding {
                secret_name: secret.name.clone(),
                env,
                value,
            });
        }
    }

    Ok(resolved)
}

fn redact_injected_secrets(text: &str, resolved: &[ResolvedSecretEnvBinding]) -> String {
    resolved.iter().fold(text.to_string(), |redacted, binding| {
        if binding.value.is_empty() {
            redacted
        } else {
            redacted.replace(&binding.value, SECRET_REDACTION)
        }
    })
}

fn injected_secret_names(resolved: &[ResolvedSecretEnvBinding]) -> Vec<String> {
    let mut names = Vec::new();
    for binding in resolved {
        if !names.contains(&binding.secret_name) {
            names.push(binding.secret_name.clone());
        }
    }
    names
}

fn injected_env_names(resolved: &[ResolvedSecretEnvBinding]) -> Vec<String> {
    resolved.iter().map(|binding| binding.env.clone()).collect()
}

fn is_valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

/// Check whether the rush backend should be used.
///
/// Returns true when the `rush-backend` feature is compiled in AND the env var
/// `IMP_SHELL_BACKEND` is either unset or set to `"rush"`. Setting
/// `IMP_SHELL_BACKEND=sh` forces the traditional `sh -c` path even when rush
/// is available.
#[cfg(feature = "rush-backend")]
fn use_rush_backend() -> bool {
    match std::env::var("IMP_SHELL_BACKEND") {
        Ok(val) => val.eq_ignore_ascii_case("rush"),
        // Feature compiled in → rush is the default.
        Err(_) => true,
    }
}

/// Execute a command via rush's in-process library API. Returns `None` if rush
/// fails so the caller can fall back to `sh`.
#[cfg(feature = "rush-backend")]
fn run_via_rush(
    command: &str,
    timeout_secs: u64,
    cwd: &std::path::Path,
    json_output: bool,
) -> Option<(String, i32, bool, bool)> {
    let result = rush::run(
        command,
        &rush::RunOptions {
            cwd: Some(cwd.to_path_buf()),
            timeout: Some(timeout_secs),
            json_output,
            max_output_bytes: Some(MAX_OUTPUT_BYTES),
            ..Default::default()
        },
    );

    match result {
        Ok(r) => {
            let mut output = r.stdout;
            if !r.stderr.is_empty() {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&r.stderr);
            }
            Some((output, r.exit_code, r.timed_out, r.truncated))
        }
        Err(_) => None,
    }
}

/// Detect which shell to use for command execution.
/// Defaults to bash, with IMP_SHELL and config.shell.command as overrides.
fn detect_shell(config: &crate::config::ShellConfig) -> String {
    // IMP_SHELL overrides everything (also used by tests to force sh)
    if let Ok(shell) = std::env::var("IMP_SHELL") {
        return shell;
    }

    if let Some(shell) = config
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return shell.to_string();
    }

    "bash".to_string()
}

fn sanitize_output_text(text: &str) -> String {
    static ANSI_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        ANSI_RE.get_or_init(|| Regex::new(r"\x1B\[[0-9;?]*[ -/]*[@-~]").expect("valid ansi regex"));
    re.replace_all(text, "").replace('\r', "")
}

fn looks_like_search_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed.starts_with("rg ")
        || trimmed == "rg"
        || trimmed.starts_with("grep ")
        || trimmed.starts_with("grep\n")
        || trimmed.starts_with("fd ")
        || trimmed == "fd"
        || trimmed.starts_with("find ")
        || trimmed == "find"
        || trimmed.starts_with("ls ")
        || trimmed == "ls"
}

fn no_match_exit_is_success(command: &str, exit_code: i32, output: &str) -> bool {
    if exit_code != 1 || !output.trim().is_empty() {
        return false;
    }

    let trimmed = command.trim_start();
    trimmed.starts_with("rg ")
        || trimmed == "rg"
        || trimmed.starts_with("grep ")
        || trimmed.starts_with("grep\n")
}

fn command_failure_hint(command: &str, exit_code: i32, output: &str) -> Option<String> {
    if no_match_exit_is_success(command, exit_code, output) {
        return Some("No matches found.".to_string());
    }

    if exit_code == 127 {
        return Some(
            "Command not found. Check the executable name or use an installed alternative."
                .to_string(),
        );
    }

    None
}

#[cfg(feature = "rush-backend")]
fn should_try_rush_json(command: &str) -> bool {
    if command.contains("|")
        || command.contains("&&")
        || command.contains("||")
        || command.contains(';')
        || command.contains('>')
        || command.contains('<')
    {
        return false;
    }

    looks_like_search_command(command)
}

#[cfg(feature = "rush-backend")]
fn parse_json_lines_to_text(command: &str, output: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    let items = value.as_array()?;

    let mut lines = Vec::new();
    let is_grep = command.trim_start().starts_with("grep");
    let is_find = command.trim_start().starts_with("find");
    let is_ls = command.trim_start().starts_with("ls");

    for item in items {
        if is_grep {
            let file = item.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let line = item
                .get("line_number")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let full_line = item
                .get("full_line")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim_end_matches('\n');
            if !file.is_empty() && line > 0 {
                lines.push(format!("{file}:{line}:{full_line}"));
            }
        } else if is_find {
            if let Some(path) = item.get("path").and_then(|v| v.as_str()) {
                lines.push(path.to_string());
            }
        } else if is_ls {
            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                let suffix = match item.get("type").and_then(|v| v.as_str()) {
                    Some("directory") => "/",
                    Some("symlink") => "@",
                    _ => "",
                };
                lines.push(format!("{name}{suffix}"));
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn truncate_command_output(command: &str, output: &str) -> TruncationResult {
    if looks_like_search_command(command) {
        truncate_head(output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES)
    } else {
        truncate_tail(output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES)
    }
}

pub struct BashTool;

impl BashTool {
    pub fn canonical() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn label(&self) -> &str {
        "Shell"
    }
    fn description(&self) -> &str {
        "Run a shell command in the workspace or an optional workdir."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout": { "type": "number" },
                "workdir": { "type": "string" },
                "with_secrets": {
                    "type": "array",
                    "description": "Imp secret names to expose as deterministic env vars.",
                    "items": { "type": "string" }
                }
            },
            "required": ["command"]
        })
    }
    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let command = params["command"]
            .as_str()
            .ok_or_else(|| crate::error::Error::Tool("missing 'command' parameter".into()))?;

        let timeout_secs = params["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_SECS);

        // Support per-command workdir override
        let ctx = if let Some(workdir) = params["workdir"].as_str() {
            let wd = super::resolve_path(&ctx.cwd, workdir);
            if !wd.is_dir() {
                return Ok(ToolOutput::error(format!(
                    "workdir not found or not a directory: {}",
                    wd.display()
                )));
            }
            ToolContext { cwd: wd, ..ctx }
        } else {
            ctx
        };

        let with_secrets = parse_with_secrets(params.get("with_secrets"))?;

        run_command(command, timeout_secs, &ctx, with_secrets).await
    }
}

async fn run_command(
    command: &str,
    timeout_secs: u64,
    ctx: &ToolContext,
    with_secrets: Vec<RequestedSecret>,
) -> Result<ToolOutput> {
    // Check cancellation before spawning.
    if ctx.is_cancelled() {
        return Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text {
                text: "[Command cancelled]".to_string(),
            }],
            details: json!({ "exit_code": -1, "timed_out": false, "cancelled": true, "truncated": false }),
            is_error: true,
        });
    }

    let resolved_secret_env = resolve_with_secrets(ctx, with_secrets)?;

    // Try the rush in-process backend when available.
    #[cfg(feature = "rush-backend")]
    if use_rush_backend() {
        let rush_json = should_try_rush_json(command);
        if let Some((output, exit_code, timed_out, truncated)) =
            run_via_rush(command, timeout_secs, &ctx.cwd, rush_json)
        {
            let transformed = if rush_json {
                parse_json_lines_to_text(command, &output).unwrap_or(output)
            } else {
                output
            };
            let sanitized = sanitize_output_text(&transformed);
            let redacted = redact_injected_secrets(&sanitized, &resolved_secret_env);
            // Stream the output lines so callers see incremental progress.
            for line in redacted.lines() {
                let _ = ctx
                    .update_tx
                    .send(ToolUpdate {
                        content: vec![imp_llm::ContentBlock::Text {
                            text: line.to_string(),
                        }],
                        details: serde_json::Value::Null,
                    })
                    .await;
            }

            let mut result_text = redacted;
            if let Some(hint) = command_failure_hint(command, exit_code, &result_text) {
                if !result_text.is_empty() {
                    result_text.push('\n');
                }
                result_text.push_str(&format!("[{hint}]"));
            }
            if timed_out {
                result_text.push_str(&format!("\n[Command timed out after {timeout_secs}s]"));
            }

            return Ok(ToolOutput {
                content: vec![imp_llm::ContentBlock::Text { text: result_text }],
                details: json!({
                    "exit_code": exit_code,
                    "timed_out": timed_out,
                    "cancelled": false,
                    "truncated": truncated,
                    "backend": "rush",
                    "with_secrets": injected_secret_names(&resolved_secret_env),
                    "injected_env": injected_env_names(&resolved_secret_env),
                }),
                is_error: timed_out
                    || (exit_code != 0
                        && !no_match_exit_is_success(command, exit_code, &transformed)),
            });
        }
        // rush failed — fall through to sh.
    }

    let mut child = {
        // Use configured shell for standard command execution.
        let shell = detect_shell(&ctx.config.shell);
        let mut cmd = Command::new(&shell);
        cmd.arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            // Tool commands are non-interactive. Keep stdin disconnected so
            // subprocesses cannot consume raw terminal input (for example SGR
            // mouse reporting sequences) from the interactive TUI.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for binding in &resolved_secret_env {
            cmd.env(&binding.env, &binding.value);
        }

        // Create a new process group so we can kill the entire tree.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        cmd.spawn()
            .map_err(|e| crate::error::Error::Tool(format!("failed to spawn command: {e}")))?
    };

    let stdout = child.stdout.take().ok_or_else(|| {
        crate::error::Error::Tool(
            "failed to capture child stdout despite stdout being piped".to_string(),
        )
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        crate::error::Error::Tool(
            "failed to capture child stderr despite stderr being piped".to_string(),
        )
    })?;

    // Merge stdout and stderr into a single stream.
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut output = String::new();
    let mut timed_out = false;
    let mut stdout_done = false;
    let mut stderr_done = false;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    while !stdout_done || !stderr_done {
        tokio::select! {
            biased;

            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                kill_process_group(&child).await;
                break;
            }

            _ = wait_for_cancellation(&ctx.cancelled), if !ctx.is_cancelled() => {
                kill_process_group(&child).await;
                break;
            }

            line = stdout_reader.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => {
                        if !line.bytes().any(|b| b == 0) {
                            let clean = sanitize_output_text(&line);
                            let clean = redact_injected_secrets(&clean, &resolved_secret_env);
                            if !clean.is_empty() {
                                append_line(&mut output, &clean, &ctx.update_tx).await;
                            }
                        }
                    }
                    _ => { stdout_done = true; }
                }
            }

            line = stderr_reader.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        if !line.bytes().any(|b| b == 0) {
                            let clean = sanitize_output_text(&line);
                            let clean = redact_injected_secrets(&clean, &resolved_secret_env);
                            if !clean.is_empty() {
                                append_line(&mut output, &clean, &ctx.update_tx).await;
                            }
                        }
                    }
                    _ => { stderr_done = true; }
                }
            }
        }
    }

    // Wait for child with a timeout — don't hang if process won't exit
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .ok()
        .and_then(|r| r.ok());
    let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);

    // Keep only redacted command output after streaming so any truncation temp
    // file cannot persist injected secret values to disk.
    output = redact_injected_secrets(&output, &resolved_secret_env);

    // Truncate from the tail (end matters more for command output).
    let TruncationResult {
        content: truncated_output,
        truncated,
        output_lines,
        total_lines,
        temp_file,
        ..
    } = truncate_command_output(command, &output);

    let mut result_text = truncated_output;

    if truncated {
        let note = if looks_like_search_command(command) {
            format!(
                "\n[Output truncated: showing first {output_lines} of {total_lines} lines{}]",
                temp_file
                    .as_ref()
                    .map(|p| format!(". Full output saved to {}", p.display()))
                    .unwrap_or_default()
            )
        } else {
            format!(
                "\n[Output truncated: showing last {output_lines} of {total_lines} lines{}]",
                temp_file
                    .as_ref()
                    .map(|p| format!(". Full output saved to {}", p.display()))
                    .unwrap_or_default()
            )
        };
        result_text.push_str(&note);
    }

    if timed_out {
        result_text.push_str(&format!("\n[Command timed out after {timeout_secs}s]"));
    }

    if let Some(hint) = command_failure_hint(command, exit_code, &output) {
        if !result_text.is_empty() {
            result_text.push('\n');
        }
        result_text.push_str(&format!("[{hint}]"));
    }

    let cancelled = ctx.is_cancelled();
    let details = json!({
        "exit_code": exit_code,
        "timed_out": timed_out,
        "cancelled": cancelled,
        "truncated": truncated,
        "command": command,
        "with_secrets": injected_secret_names(&resolved_secret_env),
        "injected_env": injected_env_names(&resolved_secret_env),
    });

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: result_text }],
        details,
        is_error: cancelled
            || timed_out
            || (exit_code != 0 && !no_match_exit_is_success(command, exit_code, &output)),
    })
}

async fn wait_for_cancellation(cancelled: &std::sync::atomic::AtomicBool) {
    while !cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn append_line(
    output: &mut String,
    line: &str,
    update_tx: &tokio::sync::mpsc::Sender<ToolUpdate>,
) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(line);
    let _ = update_tx
        .send(ToolUpdate {
            content: vec![imp_llm::ContentBlock::Text {
                text: line.to_string(),
            }],
            details: serde_json::Value::Null,
        })
        .await;
}

/// Kill the entire process group. Sends SIGTERM, waits briefly, then SIGKILL.
#[cfg(unix)]
async fn kill_process_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        let pgid = pid as i32;

        // SIGTERM the group
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }

        // Brief wait, then force-kill
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
async fn kill_process_group(_child: &tokio::process::Child) {
    // Best-effort on non-Unix — nothing we can do portably.
}

#[cfg(test)]
#[path = "bash/tests.rs"]
mod tests;
