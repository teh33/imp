use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::child_process::{isolate_tokio_command, kill_tokio_process_group};

use super::{
    VerificationArtifactRef, VerificationCommand, VerificationGate, VerificationGateKind,
    VerificationGateResult,
};
use crate::error::{Error, Result};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct VerificationGateRunner {
    cwd: PathBuf,
    artifact_root: PathBuf,
    default_timeout: Duration,
    max_capture_bytes: usize,
}

impl VerificationGateRunner {
    pub fn new(cwd: impl Into<PathBuf>, artifact_root: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            artifact_root: artifact_root.into(),
            default_timeout: DEFAULT_TIMEOUT,
            max_capture_bytes: MAX_CAPTURE_BYTES,
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

        let mut child_command = Command::new("/bin/sh");
        child_command
            .arg("-lc")
            .arg(&command.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Put verification commands in their own process group so timeout
        // cleanup can terminate grandchildren spawned by the shell too.
        isolate_tokio_command(&mut child_command);

        let mut child = child_command.spawn().map_err(Error::Io)?;

        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });

        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(wait) => wait.map_err(Error::Io)?,
            Err(_) => {
                kill_tokio_process_group(&child).await;
                let _ = child.kill().await;
                let stdout_bytes = join_output(stdout_task).await;
                let stderr_bytes = join_output(stderr_task).await;
                let result = self
                    .write_artifacts(
                        gate,
                        &command,
                        &cwd,
                        started.elapsed(),
                        None,
                        stdout_bytes,
                        stderr_bytes,
                        &gate_dir,
                        Some(format!(
                            "verification command timed out after {}ms",
                            timeout.as_millis()
                        )),
                    )
                    .await?;
                gate.mark_blocked(
                    result
                        .summary
                        .clone()
                        .unwrap_or_else(|| "verification command timed out".into()),
                );
                return Ok(result);
            }
        };

        let stdout_bytes = join_output(stdout_task).await;
        let stderr_bytes = join_output(stderr_task).await;
        let exit_code = status.code();
        let result = self
            .write_artifacts(
                gate,
                &command,
                &cwd,
                started.elapsed(),
                exit_code,
                stdout_bytes,
                stderr_bytes,
                &gate_dir,
                None,
            )
            .await?;

        match exit_code {
            Some(0) => gate.mark_passed(result.clone()),
            _ => gate.mark_failed(result.clone()),
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
        stdout_bytes: Vec<u8>,
        stderr_bytes: Vec<u8>,
        gate_dir: &Path,
        blocked_summary: Option<String>,
    ) -> Result<VerificationGateResult> {
        let stdout_capture = CapturedOutput::new(stdout_bytes, self.max_capture_bytes);
        let stderr_capture = CapturedOutput::new(stderr_bytes, self.max_capture_bytes);
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

async fn join_output(task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) -> Vec<u8> {
    match task.await {
        Ok(Ok(bytes)) => bytes,
        _ => Vec::new(),
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
    fn new(bytes: Vec<u8>, max_bytes: usize) -> Self {
        let original_len = bytes.len();
        let truncated = original_len > max_bytes;
        let slice = if truncated {
            &bytes[..max_bytes]
        } else {
            &bytes[..]
        };
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
mod tests {
    use super::*;
    use crate::workflow::VerificationGateStatus;

    #[tokio::test]
    async fn command_gate_runner_passes_and_writes_artifacts() {
        let temp = tempfile::TempDir::new().unwrap();
        let runner = VerificationGateRunner::new(temp.path(), temp.path().join("artifacts"));
        let mut gate = VerificationGate::command("pass", "printf 'hello' && printf 'warn' >&2");

        let result = runner.run(&mut gate).await.unwrap();

        assert_eq!(gate.status, VerificationGateStatus::Passed);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout_summary.as_deref(), Some("hello"));
        assert_eq!(result.stderr_summary.as_deref(), Some("warn"));
        assert!(gate
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "stdout"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("artifacts/pass/stdout.log")).unwrap(),
            "hello"
        );
        assert!(temp.path().join("artifacts/pass/status.json").exists());
    }

    #[tokio::test]
    async fn command_gate_runner_marks_failed_command() {
        let temp = tempfile::TempDir::new().unwrap();
        let runner = VerificationGateRunner::new(temp.path(), temp.path().join("artifacts"));
        let mut gate = VerificationGate::command("fail", "printf 'bad' >&2; exit 7");

        let result = runner.run(&mut gate).await.unwrap();

        assert_eq!(gate.status, VerificationGateStatus::Failed);
        assert_eq!(result.exit_code, Some(7));
        assert!(result.summary.unwrap().contains("exit code 7"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("artifacts/fail/stderr.log")).unwrap(),
            "bad"
        );
    }

    #[tokio::test]
    async fn command_gate_runner_marks_timeout_blocked() {
        let temp = tempfile::TempDir::new().unwrap();
        let runner = VerificationGateRunner::new(temp.path(), temp.path().join("artifacts"))
            .with_default_timeout(Duration::from_millis(50));
        let mut gate = VerificationGate::command("timeout", "sleep 2");

        let result = runner.run(&mut gate).await.unwrap();

        assert_eq!(gate.status, VerificationGateStatus::Blocked);
        assert_eq!(result.exit_code, None);
        assert!(result.summary.unwrap().contains("timed out"));
        assert!(temp.path().join("artifacts/timeout/status.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_gate_runner_writes_private_artifacts() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let artifact_dir = temp.path().join("artifacts");
        let runner = VerificationGateRunner::new(temp.path(), &artifact_dir);
        let mut gate = VerificationGate::command("private-artifacts", "echo secret-ish");

        let result = runner.run(&mut gate).await.unwrap();
        assert_eq!(result.exit_code, Some(0));

        for artifact in &gate.artifacts {
            let mode = std::fs::metadata(&artifact.path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{}", artifact.path.display());
        }
    }

    #[tokio::test]
    async fn command_gate_runner_truncates_large_output() {
        let temp = tempfile::TempDir::new().unwrap();
        let runner = VerificationGateRunner::new(temp.path(), temp.path().join("artifacts"))
            .with_max_capture_bytes(5);
        let mut gate = VerificationGate::command("truncate", "printf 'abcdefghijklmnopqrstuvwxyz'");

        let result = runner.run(&mut gate).await.unwrap();

        assert_eq!(gate.status, VerificationGateStatus::Passed);
        assert!(result.stdout_summary.unwrap().contains("truncated"));
        let stdout =
            std::fs::read_to_string(temp.path().join("artifacts/truncate/stdout.log")).unwrap();
        assert!(stdout.starts_with("abcde"));
        assert!(stdout.contains("truncated"));
        assert!(gate
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "stdout"
                && artifact.redaction.as_deref() == Some("output truncated")));
    }
}
