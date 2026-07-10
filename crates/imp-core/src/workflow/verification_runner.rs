use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    VerificationArtifactRef, VerificationCommand, VerificationGate, VerificationGateKind,
    VerificationGateResult,
};
use crate::error::{Error, Result};
use crate::process::{ProcessError, ProcessManager};

use super::verification_runner_execution::{execute, VerificationExecution};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct VerificationGateRunner {
    cwd: PathBuf,
    artifact_root: PathBuf,
    default_timeout: Duration,
    max_capture_bytes: usize,
    manager: ProcessManager,
}

impl fmt::Debug for VerificationGateRunner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerificationGateRunner")
            .field("cwd", &self.cwd)
            .field("artifact_root", &self.artifact_root)
            .field("default_timeout", &self.default_timeout)
            .field("max_capture_bytes", &self.max_capture_bytes)
            .finish()
    }
}

impl VerificationGateRunner {
    pub fn new(cwd: impl Into<PathBuf>, artifact_root: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            artifact_root: artifact_root.into(),
            default_timeout: DEFAULT_TIMEOUT,
            max_capture_bytes: MAX_CAPTURE_BYTES,
            manager: ProcessManager::new(),
        }
    }

    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn with_max_capture_bytes(mut self, bytes: usize) -> Self {
        self.max_capture_bytes = bytes;
        self
    }

    pub async fn run(&self, gate: &mut VerificationGate) -> Result<VerificationGateResult> {
        let Some(command) = gate.command.clone() else {
            gate.mark_blocked("verification gate has no command");
            return Ok(VerificationGateResult {
                summary: Some("verification gate has no command".into()),
                ..VerificationGateResult::default()
            });
        };
        if gate.kind != VerificationGateKind::Command {
            gate.mark_blocked("only command verification gates are executable today");
            return Ok(VerificationGateResult {
                summary: Some("only command verification gates are executable today".into()),
                ..VerificationGateResult::default()
            });
        }

        gate.mark_running();
        let started = Instant::now();
        let cwd = command.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let timeout = command.timeout.unwrap_or(self.default_timeout);
        let gate_dir = self.artifact_root.join(sanitize_path_segment(&gate.id));
        tokio::fs::create_dir_all(&gate_dir)
            .await
            .map_err(Error::Io)?;

        let execution = execute(
            &self.manager,
            &command.command,
            &cwd,
            timeout,
            self.max_capture_bytes,
        )
        .await
        .map_err(process_error)?;
        let timed_out = execution.exit.timed_out;
        let exit_code = (!timed_out).then_some(execution.exit.code).flatten();
        let blocked_summary = timed_out.then(|| {
            format!(
                "verification command timed out after {}ms",
                timeout.as_millis()
            )
        });
        let result = self
            .write_artifacts(
                gate,
                &command,
                &cwd,
                started.elapsed(),
                exit_code,
                execution,
                &gate_dir,
                blocked_summary,
            )
            .await?;

        if timed_out {
            gate.mark_blocked(
                result
                    .summary
                    .clone()
                    .unwrap_or_else(|| "verification command timed out".into()),
            );
        } else {
            match exit_code {
                Some(0) => gate.mark_passed(result.clone()),
                _ => gate.mark_failed(result.clone()),
            }
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_artifacts(
        &self,
        gate: &mut VerificationGate,
        command: &VerificationCommand,
        cwd: &Path,
        elapsed: Duration,
        exit_code: Option<i32>,
        execution: VerificationExecution,
        gate_dir: &Path,
        blocked_summary: Option<String>,
    ) -> Result<VerificationGateResult> {
        let stdout_capture = CapturedOutput::new(
            execution.stdout,
            execution.stdout_bytes,
            self.max_capture_bytes,
        );
        let stderr_capture = CapturedOutput::new(
            execution.stderr,
            execution.stderr_bytes,
            self.max_capture_bytes,
        );
        let stdout_path = gate_dir.join("stdout.log");
        let stderr_path = gate_dir.join("stderr.log");
        let status_path = gate_dir.join("status.json");

        write_private_file(&stdout_path, stdout_capture.content.as_bytes()).await?;
        write_private_file(&stderr_path, stderr_capture.content.as_bytes()).await?;

        let summary = blocked_summary.unwrap_or_else(|| match exit_code {
            Some(0) => "verification command passed".to_string(),
            Some(code) => format!("verification command failed with exit code {code}"),
            None => "verification command terminated without exit code".to_string(),
        });

        let result = VerificationGateResult {
            exit_code,
            duration_ms: Some(elapsed.as_millis() as u64),
            summary: Some(summary),
            stdout_summary: Some(stdout_capture.summary()),
            stderr_summary: Some(stderr_capture.summary()),
        };

        let status_json = serde_json::json!({
            "gate_id": gate.id,
            "command": command.command,
            "cwd": cwd,
            "exit_code": exit_code,
            "duration_ms": result.duration_ms,
            "summary": result.summary,
            "stdout_truncated": stdout_capture.truncated,
            "stderr_truncated": stderr_capture.truncated,
        });
        write_private_file(
            &status_path,
            &serde_json::to_vec_pretty(&status_json).map_err(Error::Json)?,
        )
        .await?;

        gate.artifacts = vec![
            artifact_ref(
                "stdout",
                stdout_path,
                stdout_capture.original_len,
                stdout_capture.truncated,
            ),
            artifact_ref(
                "stderr",
                stderr_path,
                stderr_capture.original_len,
                stderr_capture.truncated,
            ),
            artifact_ref("status", status_path, None, false),
        ];
        Ok(result)
    }
}

async fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    tokio::fs::write(path, contents).await.map_err(Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(Error::Io)?;
    }
    Ok(())
}

fn artifact_ref(
    kind: &str,
    path: PathBuf,
    bytes: Option<usize>,
    truncated: bool,
) -> VerificationArtifactRef {
    let mut artifact = VerificationArtifactRef::new(kind, path);
    artifact.bytes = bytes.map(|bytes| bytes as u64);
    if truncated {
        artifact.redaction = Some("output truncated".into());
    }
    artifact
}

fn process_error(error: ProcessError) -> Error {
    match error {
        ProcessError::Spawn(error) | ProcessError::Io(error) => Error::Io(error),
        error => Error::Tool(error.to_string()),
    }
}

fn sanitize_path_segment(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "gate".into()
    } else {
        sanitized
    }
}

struct CapturedOutput {
    content: String,
    original_len: Option<usize>,
    truncated: bool,
}

impl CapturedOutput {
    fn new(bytes: Vec<u8>, original_len: usize, max_bytes: usize) -> Self {
        let truncated = original_len > max_bytes;
        let retained_len = bytes.len().min(max_bytes);
        let slice = &bytes[..retained_len];
        let mut content = String::from_utf8_lossy(slice).to_string();
        if truncated {
            content.push_str("\n[verification output truncated]\n");
        }
        Self {
            content,
            original_len: Some(original_len),
            truncated,
        }
    }

    fn summary(&self) -> String {
        let trimmed = self.content.trim();
        if trimmed.is_empty() {
            return "<empty>".into();
        }
        let mut summary: String = trimmed.chars().take(500).collect();
        if trimmed.chars().count() > 500 {
            summary.push('…');
        }
        summary
    }
}

#[cfg(test)]
#[path = "verification_runner_tests.rs"]
mod tests;
