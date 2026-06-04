use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use glob::Pattern;
use imp_llm::{AssistantMessage, ContentBlock, Message, ToolResultMessage};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const HOOK_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Reports outcomes from background non-blocking hook execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookBackgroundEvent {
    NonBlockingHookFailed {
        event: String,
        command: String,
        error: String,
    },
    NonBlockingHookPanicked {
        event: String,
        command: String,
        error: String,
    },
}

impl std::fmt::Display for HookBackgroundEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonBlockingHookFailed {
                event,
                command,
                error,
            } => write!(
                f,
                "Non-blocking hook failed for event '{event}' while running `{command}`: {error}"
            ),
            Self::NonBlockingHookPanicked {
                event,
                command,
                error,
            } => write!(
                f,
                "Non-blocking hook panicked for event '{event}' while running `{command}`: {error}"
            ),
        }
    }
}

/// Hook definition from TOML config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    pub event: String,
    #[serde(rename = "match")]
    pub match_pattern: Option<String>,
    pub action: String,
    pub command: Option<String>,
    #[serde(default)]
    pub blocking: bool,
    pub threshold: Option<f64>,
}

/// What a hook does when triggered.
#[derive(Clone)]
pub enum HookAction {
    /// Run a shell command with interpolation ({file}, {tool_name}).
    Shell { command: String },
    /// A programmatic callback (for Lua or other extensions).
    Callback(Arc<dyn Fn(&HookEvent<'_>) -> HookResult + Send + Sync>),
}

impl std::fmt::Debug for HookAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookAction::Shell { command } => {
                f.debug_struct("Shell").field("command", command).finish()
            }
            HookAction::Callback(_) => f.write_str("Callback(...)"),
        }
    }
}

/// A fully resolved hook definition ready for execution.
#[derive(Debug, Clone)]
pub struct HookDefinition {
    pub event: String,
    pub match_pattern: Option<String>,
    pub action: HookAction,
    pub blocking: bool,
    pub threshold: Option<f64>,
}

/// Runtime hook events.
#[derive(Clone)]
pub enum HookEvent<'a> {
    AfterFileWrite {
        file: &'a Path,
    },
    BeforeToolCall {
        tool_name: &'a str,
        args: &'a serde_json::Value,
    },
    AfterToolCall {
        tool_name: &'a str,
        result: &'a ToolResultMessage,
    },
    BeforeLlmCall,
    OnContextThreshold {
        ratio: f64,
    },
    OnSessionStart,
    OnSessionShutdown,
    OnAgentStart {
        prompt: &'a str,
    },
    OnAgentEnd {
        messages: &'a [Message],
    },
    OnTurnEnd {
        index: u32,
        message: &'a AssistantMessage,
    },
}

impl<'a> HookEvent<'a> {
    /// Return the canonical event name for matching against hook definitions.
    fn event_name(&self) -> &'static str {
        match self {
            HookEvent::AfterFileWrite { .. } => "after_file_write",
            HookEvent::BeforeToolCall { .. } => "before_tool_call",
            HookEvent::AfterToolCall { .. } => "after_tool_call",
            HookEvent::BeforeLlmCall => "before_llm_call",
            HookEvent::OnContextThreshold { .. } => "on_context_threshold",
            HookEvent::OnSessionStart => "on_session_start",
            HookEvent::OnSessionShutdown => "on_session_shutdown",
            HookEvent::OnAgentStart { .. } => "on_agent_start",
            HookEvent::OnAgentEnd { .. } => "on_agent_end",
            HookEvent::OnTurnEnd { .. } => "on_turn_end",
        }
    }
}

/// Result from a hook execution.
#[derive(Default, Debug)]
pub struct HookResult {
    pub block: bool,
    pub reason: Option<String>,
    pub modified_content: Option<Vec<ContentBlock>>,
}

/// Manages and executes hooks.
pub struct HookRunner {
    /// TOML-defined hooks (fire first, in config order).
    toml_hooks: Vec<HookDefinition>,
    /// Programmatically registered hooks (fire after TOML hooks, in registration order).
    programmatic_hooks: Vec<HookDefinition>,
    /// Optional observer for background non-blocking hook failures.
    background_reporter: Option<Arc<dyn Fn(HookBackgroundEvent) + Send + Sync>>,
}

impl HookRunner {
    pub fn new() -> Self {
        Self {
            toml_hooks: Vec::new(),
            programmatic_hooks: Vec::new(),
            background_reporter: None,
        }
    }

    /// Add a single TOML hook def (raw from config).
    pub fn add(&mut self, def: HookDef) {
        if let Some(resolved) = resolve_hook_def(def) {
            self.toml_hooks.push(resolved);
        }
    }

    /// Load multiple TOML hook defs from config.
    pub fn load_from_config(&mut self, defs: Vec<HookDef>) {
        for def in defs {
            self.add(def);
        }
    }

    /// Register a programmatic hook (for Lua or other extensions).
    pub fn register(&mut self, hook: HookDefinition) {
        self.programmatic_hooks.push(hook);
    }

    /// Returns the total number of registered hooks (TOML + programmatic).
    pub fn len(&self) -> usize {
        self.toml_hooks.len() + self.programmatic_hooks.len()
    }

    /// Returns true if no hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.toml_hooks.is_empty() && self.programmatic_hooks.is_empty()
    }

    /// Register an observer for background non-blocking hook failures.
    pub fn set_background_reporter(
        &mut self,
        reporter: Arc<dyn Fn(HookBackgroundEvent) + Send + Sync>,
    ) {
        self.background_reporter = Some(reporter);
    }

    /// Register a callback hook for a specific event.
    pub fn register_callback(
        &mut self,
        event: &str,
        callback: Arc<dyn Fn(&HookEvent<'_>) -> HookResult + Send + Sync>,
    ) {
        self.programmatic_hooks.push(HookDefinition {
            event: event.to_string(),
            match_pattern: None,
            action: HookAction::Callback(callback),
            blocking: true,
            threshold: None,
        });
    }

    /// Fire a hook event and collect results.
    ///
    /// Execution order: TOML hooks first (config order), then programmatic hooks (registration order).
    /// Blocking hooks execute sequentially and await completion.
    /// Non-blocking hooks are spawned as background tokio tasks.
    pub async fn fire(&self, event: &HookEvent<'_>) -> Vec<HookResult> {
        let mut results = Vec::new();

        // TOML hooks first, then programmatic hooks
        let all_hooks = self.toml_hooks.iter().chain(self.programmatic_hooks.iter());

        for hook in all_hooks {
            if !matches_event(hook, event) {
                continue;
            }

            if hook.blocking {
                let result = execute_hook(hook, event).await;
                results.push(result);
            } else {
                // Keep non-blocking hooks asynchronous, but supervise failures.
                if let HookAction::Shell { command } = &hook.action {
                    let cmd = interpolate_command(command, event);
                    run_non_blocking_shell_hook(
                        hook_event_label(event),
                        cmd,
                        self.background_reporter.clone(),
                    );
                }
                // Non-blocking hooks don't contribute results
            }
        }

        results
    }
}

impl Default for HookRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn hook_event_label(event: &HookEvent<'_>) -> String {
    event.event_name().to_string()
}

fn report_non_blocking_hook_outcome(
    join_result: Result<std::io::Result<std::process::Output>, tokio::task::JoinError>,
    event_name: String,
    command_for_report: String,
    reporter: Arc<dyn Fn(HookBackgroundEvent) + Send + Sync>,
) {
    match join_result {
        Ok(Ok(output)) => {
            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let error = if !stderr.is_empty() {
                    stderr
                } else if !stdout.is_empty() {
                    stdout
                } else {
                    format!(
                        "command exited with status {}",
                        output
                            .status
                            .code()
                            .map(|code| code.to_string())
                            .unwrap_or_else(|| "terminated by signal".into())
                    )
                };
                reporter(HookBackgroundEvent::NonBlockingHookFailed {
                    event: event_name,
                    command: command_for_report,
                    error,
                });
            }
        }
        Ok(Err(error)) => reporter(HookBackgroundEvent::NonBlockingHookFailed {
            event: event_name,
            command: command_for_report,
            error: error.to_string(),
        }),
        Err(join_error) => reporter(HookBackgroundEvent::NonBlockingHookPanicked {
            event: event_name,
            command: command_for_report,
            error: join_error.to_string(),
        }),
    }
}

fn run_non_blocking_shell_hook(
    event_name: String,
    command: String,
    reporter: Option<Arc<dyn Fn(HookBackgroundEvent) + Send + Sync>>,
) {
    tokio::spawn(async move {
        let command_for_run = command.clone();
        let command_for_report = command;
        let join_result = tokio::spawn(async move {
            run_hook_shell_command(&command_for_run, HOOK_COMMAND_TIMEOUT).await
        })
        .await;

        if let Some(reporter) = reporter {
            report_non_blocking_hook_outcome(join_result, event_name, command_for_report, reporter);
        }
    });
}

async fn run_hook_shell_command(
    command_text: &str,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(command_text)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status_result) => {
            let status = status_result?;
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            if let Some(mut stream) = stdout.take() {
                stream.read_to_end(&mut stdout_bytes).await?;
            }
            if let Some(mut stream) = stderr.take() {
                stream.read_to_end(&mut stderr_bytes).await?;
            }
            Ok(std::process::Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            })
        }
        Err(_) => {
            kill_process_group(&child).await;
            let _ = child.kill().await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("hook command timed out after {}s", timeout.as_secs()),
            ))
        }
    }
}

async fn kill_process_group(child: &tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

fn resolve_hook_def(def: HookDef) -> Option<HookDefinition> {
    let action = match def.action.as_str() {
        "shell" => {
            let command = def.command?;
            HookAction::Shell { command }
        }
        _ => return None,
    };

    Some(HookDefinition {
        event: def.event,
        match_pattern: def.match_pattern,
        action,
        blocking: def.blocking,
        threshold: def.threshold,
    })
}

/// Check if a hook definition matches the given event.
fn matches_event(hook: &HookDefinition, event: &HookEvent<'_>) -> bool {
    // Event name must match
    if hook.event != event.event_name() {
        return false;
    }

    // Check match_pattern if present
    if let Some(pattern) = &hook.match_pattern {
        match event {
            HookEvent::AfterFileWrite { file } => {
                let file_str = file.to_string_lossy();
                // Try glob matching against the full path and filename
                if let Ok(glob) = Pattern::new(pattern) {
                    let file_name = file
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if !glob.matches(&file_str) && !glob.matches(&file_name) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            HookEvent::BeforeToolCall { tool_name, .. }
            | HookEvent::AfterToolCall { tool_name, .. } => {
                if pattern != *tool_name {
                    // Also try glob matching on tool name
                    if let Ok(glob) = Pattern::new(pattern) {
                        if !glob.matches(tool_name) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            _ => {
                // Other events ignore match_pattern
            }
        }
    }

    // Check threshold for OnContextThreshold
    if let HookEvent::OnContextThreshold { ratio } = event {
        if let Some(threshold) = hook.threshold {
            if *ratio < threshold {
                return false;
            }
        }
    }

    true
}

/// Interpolate variables into a shell command string.
fn interpolate_command(command: &str, event: &HookEvent<'_>) -> String {
    let mut result = command.to_string();

    match event {
        HookEvent::AfterFileWrite { file } => {
            result = replace_placeholder(&result, "file", &file.to_string_lossy());
        }
        HookEvent::BeforeToolCall { tool_name, .. } => {
            result = replace_placeholder(&result, "tool_name", tool_name);
        }
        HookEvent::AfterToolCall {
            tool_name,
            result: tool_result,
        } => {
            result = replace_placeholder(&result, "tool_name", tool_name);
            result = replace_placeholder(
                &result,
                "is_error",
                if tool_result.is_error {
                    "true"
                } else {
                    "false"
                },
            );
            // Extract exit_code from details if present (bash tool sets this)
            let exit_code = tool_result
                .details
                .get("exit_code")
                .and_then(|v| v.as_i64())
                .map(|c| c.to_string())
                .unwrap_or_default();
            result = replace_placeholder(&result, "exit_code", &exit_code);
            // First line of output for summary
            let output_first = tool_result
                .content
                .iter()
                .filter_map(|b| match b {
                    imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .next()
                .and_then(|t| t.lines().next())
                .unwrap_or("");
            result = replace_placeholder(&result, "output_first_line", output_first);
            // Extract command from details (bash tool stores it)
            let command = tool_result
                .details
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            result = replace_placeholder(&result, "command", command);
        }
        HookEvent::OnContextThreshold { ratio } => {
            result = replace_placeholder(&result, "ratio", &ratio.to_string());
        }
        HookEvent::OnTurnEnd { index, .. } => {
            result = replace_placeholder(&result, "index", &index.to_string());
        }
        _ => {}
    }

    result
}

fn replace_placeholder(template: &str, name: &str, value: &str) -> String {
    let raw = format!("{{{name}}}");
    let single_marker = format!("\u{0}__imp_hook_single_{name}__\u{0}");
    let double_marker = format!("\u{0}__imp_hook_double_{name}__\u{0}");

    let mut result = template.replace(&format!("'{raw}'"), &single_marker);
    result = result.replace(&format!("\"{raw}\""), &double_marker);
    result = result.replace(&raw, value);
    result = result.replace(&single_marker, &shell_single_quote(value));
    result = result.replace(&double_marker, &shell_double_quote(value));
    result
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_double_quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    format!("\"{escaped}\"")
}

/// Execute a single hook and return its result.
async fn execute_hook(hook: &HookDefinition, event: &HookEvent<'_>) -> HookResult {
    match &hook.action {
        HookAction::Shell { command } => {
            let cmd = interpolate_command(command, event);
            match run_hook_shell_command(&cmd, HOOK_COMMAND_TIMEOUT).await {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    // A non-zero exit code on a BeforeToolCall hook means "block"
                    let block = matches!(event, HookEvent::BeforeToolCall { .. })
                        && !output.status.success();

                    let reason = if block {
                        Some(if stderr.is_empty() {
                            stdout.clone()
                        } else {
                            stderr
                        })
                    } else {
                        None
                    };

                    // For AfterToolCall, stdout is treated as modified content
                    let modified_content = if matches!(event, HookEvent::AfterToolCall { .. })
                        && !stdout.trim().is_empty()
                        && output.status.success()
                    {
                        Some(vec![ContentBlock::Text {
                            text: stdout.trim().to_string(),
                        }])
                    } else {
                        None
                    };

                    HookResult {
                        block,
                        reason,
                        modified_content,
                    }
                }
                Err(e) => HookResult {
                    block: false,
                    reason: Some(format!("Hook command failed: {e}")),
                    modified_content: None,
                },
            }
        }
        HookAction::Callback(cb) => cb(event),
    }
}

#[cfg(test)]
#[path = "hooks/tests.rs"]
mod tests;
