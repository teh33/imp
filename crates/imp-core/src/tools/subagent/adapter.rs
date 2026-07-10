use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::model::{LooprPoll, LooprRun, LooprSent, LooprThread, SubagentMapping};
use crate::agent::SubagentInput;
use crate::error::{Error, Result};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub(super) struct LooprSubagentExecutor {
    executable: PathBuf,
    command_timeout: Duration,
}

impl Default for LooprSubagentExecutor {
    fn default() -> Self {
        Self::new(
            std::env::var_os("IMP_LOOPR_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| "loopr".into()),
        )
    }
}

impl LooprSubagentExecutor {
    pub(super) fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            command_timeout: COMMAND_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub(super) fn with_timeout(executable: PathBuf, command_timeout: Duration) -> Self {
        Self {
            executable,
            command_timeout,
        }
    }

    pub(super) async fn launch(
        &self,
        input: &SubagentInput,
        cwd: &Path,
        existing_run_id: Option<String>,
    ) -> Result<SubagentMapping> {
        let run_id = match existing_run_id {
            Some(run_id) => run_id,
            None => {
                self.json::<LooprRun>(cwd, ["start", "--json"], input.parent_run_id.as_str())
                    .await?
                    .id
            }
        };
        let prompt = child_prompt(input);
        let mut args = vec![
            "thread".to_string(),
            "spawn".to_string(),
            "--json".to_string(),
            "--run".to_string(),
            run_id.clone(),
            "--cwd".to_string(),
            cwd.display().to_string(),
            "--tag".to_string(),
            role_name(&input.role).to_string(),
            "--detach".to_string(),
        ];
        args.push(prompt);
        let thread: LooprThread = self.json_args(cwd, args).await?;
        Ok(SubagentMapping::from_launch(input, run_id, &thread))
    }

    pub(super) async fn poll(&self, mapping: &SubagentMapping, cwd: &Path) -> Result<LooprPoll> {
        self.json_args(cwd, thread_args("poll", mapping, None))
            .await
    }

    pub(super) async fn wait(
        &self,
        mapping: &SubagentMapping,
        cwd: &Path,
        timeout: u64,
    ) -> Result<LooprPoll> {
        self.json_args(cwd, thread_args("wait", mapping, Some(timeout)))
            .await
    }

    pub(super) async fn send(
        &self,
        mapping: &SubagentMapping,
        cwd: &Path,
        message: &str,
    ) -> Result<LooprSent> {
        let mut args = thread_args("send", mapping, None);
        args.push(message.to_string());
        self.json_args(cwd, args).await
    }

    pub(super) async fn stop(&self, mapping: &SubagentMapping, cwd: &Path) -> Result<LooprPoll> {
        let _: serde_json::Value = self
            .json_args(cwd, thread_args("stop", mapping, None))
            .await?;
        self.poll(mapping, cwd).await
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        cwd: &Path,
        prefix: [&str; 2],
        value: &str,
    ) -> Result<T> {
        self.json_args(cwd, vec![prefix[0].into(), prefix[1].into(), value.into()])
            .await
    }

    async fn json_args<T: serde::de::DeserializeOwned>(
        &self,
        cwd: &Path,
        args: Vec<String>,
    ) -> Result<T> {
        let output = self.run(cwd, &args).await?;
        serde_json::from_slice(&output.stdout).map_err(|error| {
            Error::Tool(format!(
                "loopr returned malformed JSON: {error}; stdout={}",
                bounded_text(&output.stdout, 4096)
            ))
        })
    }

    async fn run(&self, cwd: &Path, args: &[String]) -> Result<CommandOutput> {
        let mut command = Command::new(&self.executable);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            Error::Tool(format!(
                "failed to start loopr executable {}: {error}",
                self.executable.display()
            ))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Tool("loopr stdout was unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Tool("loopr stderr was unavailable".into()))?;
        let stdout_task = tokio::spawn(read_limited(stdout));
        let stderr_task = tokio::spawn(read_limited(stderr));
        let status = match tokio::time::timeout(self.command_timeout, child.wait()).await {
            Ok(result) => result.map_err(Error::from)?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(Error::Tool(format!(
                    "loopr command timed out after {} seconds",
                    self.command_timeout.as_secs()
                )));
            }
        };
        let stdout = stdout_task
            .await
            .map_err(|error| Error::Tool(format!("loopr stdout task failed: {error}")))??;
        let stderr = stderr_task
            .await
            .map_err(|error| Error::Tool(format!("loopr stderr task failed: {error}")))??;
        if !status.success() {
            return Err(Error::Tool(format!(
                "loopr exited with {status}: {}",
                bounded_text(&stderr, 4096)
            )));
        }
        Ok(CommandOutput { stdout, stderr })
    }
}

struct CommandOutput {
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
}

async fn read_limited<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(data);
        }
        if data.len().saturating_add(count) > MAX_OUTPUT_BYTES {
            return Err(Error::Tool(format!(
                "loopr output exceeded {MAX_OUTPUT_BYTES} byte limit"
            )));
        }
        data.extend_from_slice(&chunk[..count]);
    }
}

fn thread_args(action: &str, mapping: &SubagentMapping, timeout: Option<u64>) -> Vec<String> {
    let mut args = vec![
        "thread".into(),
        action.into(),
        mapping.loopr_thread_id.clone(),
        "--json".into(),
        "--run".into(),
        mapping.loopr_run_id.clone(),
    ];
    if let Some(timeout) = timeout {
        args.extend(["--timeout-seconds".into(), timeout.to_string()]);
    }
    args
}

fn child_prompt(input: &SubagentInput) -> String {
    let paths = |items: &[PathBuf]| {
        items
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let files = input
        .context
        .files
        .iter()
        .map(|file| {
            format!(
                "- {}{}",
                file.path.display(),
                file.note
                    .as_deref()
                    .map(|note| format!(": {note}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Role: {}\nObjective: {}\nInstructions:\n{}\nContext messages:\n{}\nContext files:\n{}\nAllowed paths: {}\nWritable paths: {}\nResource limits: timeout_seconds={:?}, max_model_tokens={:?}, max_tool_calls={:?}, max_parallel_children={:?}\nOutput contract: {}\nMerge policy: {:?}\n",
        role_name(&input.role), input.objective, input.context.instructions.join("\n"), input.context.messages.join("\n"),
        files, paths(&input.resource_limits.allowed_paths), paths(&input.resource_limits.writable_paths),
        input.resource_limits.timeout_seconds, input.resource_limits.max_model_tokens,
        input.resource_limits.max_tool_calls, input.resource_limits.max_parallel_children,
        input.output_contract.as_deref().unwrap_or("Report a structured final outcome."), input.merge_policy)
}

fn role_name(role: &crate::agent::SubagentRole) -> &str {
    match role {
        crate::agent::SubagentRole::Searcher => "searcher",
        crate::agent::SubagentRole::Planner => "planner",
        crate::agent::SubagentRole::Implementer => "implementer",
        crate::agent::SubagentRole::Verifier => "verifier",
        crate::agent::SubagentRole::Reviewer => "reviewer",
        crate::agent::SubagentRole::Synthesizer => "synthesizer",
        crate::agent::SubagentRole::Custom(value) => value,
    }
}

fn bounded_text(bytes: &[u8], limit: usize) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(limit)])
        .trim()
        .to_string()
}
