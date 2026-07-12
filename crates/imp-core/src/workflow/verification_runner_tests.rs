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
async fn command_gate_runner_closes_stdin() {
    let temp = tempfile::TempDir::new().unwrap();
    let runner = VerificationGateRunner::new(temp.path(), temp.path().join("artifacts"));
    let mut gate = VerificationGate::command(
        "stdin-eof",
        "if IFS= read -r line; then printf unexpected; else printf eof; fi",
    );

    let result = runner.run(&mut gate).await.unwrap();

    assert_eq!(gate.status, VerificationGateStatus::Passed);
    assert_eq!(result.stdout_summary.as_deref(), Some("eof"));
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
