use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use super::*;

fn grant(cwd: &Path) -> ExecutionGrant {
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = std::env::vars().map(|(name, _)| name).collect();
    grant
}

fn shell_request(cwd: &Path, script: &str) -> ProcessRequest {
    ProcessRequest {
        command: CommandSpec::new("sh").with_arguments(["-c".into(), script.into()]),
        cwd: cwd.to_path_buf(),
        environment: std::env::vars().collect::<BTreeMap<_, _>>(),
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(Duration::from_secs(5)),
        output_retention_bytes: 64 * 1024,
        grant: grant(cwd),
    }
}

#[tokio::test]
async fn process_start_drains_both_streams_and_repeated_wait() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let info = manager
        .start(shell_request(
            dir.path(),
            "printf out; printf err >&2; exit 7",
        ))
        .await
        .unwrap();
    let first = manager.wait(info.id).await.unwrap();
    let second = manager.wait(info.id).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first.code, Some(7));
    let output = manager
        .read(info.id, OutputCursor::start(info.id), usize::MAX)
        .await
        .unwrap();
    assert!(output
        .chunks
        .iter()
        .any(|chunk| chunk.stream == OutputStream::Stdout && chunk.text.contains("out")));
    assert!(output
        .chunks
        .iter()
        .any(|chunk| chunk.stream == OutputStream::Stderr && chunk.text.contains("err")));
    assert_eq!(manager.get(info.id).unwrap().state, ProcessState::Exited);
    assert_eq!(manager.list().len(), 1);
}

#[tokio::test]
async fn cursor_reads_incrementally_and_reports_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let mut request = shell_request(dir.path(), "printf abcdefgh");
    request.output_retention_bytes = 4;
    let info = manager.start(request).await.unwrap();
    manager.wait(info.id).await.unwrap();
    let output = manager
        .read(info.id, OutputCursor::start(info.id), 2)
        .await
        .unwrap();
    assert!(output.unread_output_evicted);
    assert!(output.response_truncated);
    assert_eq!(output.chunks[0].text, "ef");
    let rest = manager.read(info.id, output.next_cursor, 10).await.unwrap();
    assert_eq!(rest.chunks[0].text, "gh");
}

#[tokio::test]
async fn stdin_write_and_write_after_exit_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let info = manager
        .start(shell_request(
            dir.path(),
            "IFS= read -r line; printf '%s' \"$line\"",
        ))
        .await
        .unwrap();
    manager.write(info.id, b"hello\n").await.unwrap();
    manager.wait(info.id).await.unwrap();
    let output = manager
        .read(info.id, OutputCursor::start(info.id), 100)
        .await
        .unwrap();
    assert!(output.chunks.iter().any(|chunk| chunk.text == "hello"));
    assert!(
        matches!(manager.write(info.id, b"late").await, Err(ProcessError::NotRunning(id)) if id == info.id)
    );
}

#[tokio::test]
async fn timeout_and_repeated_stop_are_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let mut request = shell_request(dir.path(), "exec sleep 60");
    request.timeout = Some(Duration::from_millis(50));
    let info = manager.start(request).await.unwrap();
    let exit = manager.wait(info.id).await.unwrap();
    assert!(exit.timed_out);
    assert_eq!(manager.get(info.id).unwrap().state, ProcessState::TimedOut);
    assert_eq!(
        manager.stop(info.id, StopOptions::default()).await.unwrap(),
        exit
    );
}

#[tokio::test]
async fn graceful_stop_and_cancel_are_classified() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let stopped = manager
        .start(shell_request(dir.path(), "exec sleep 60"))
        .await
        .unwrap();
    let exit = manager
        .stop(stopped.id, StopOptions::default())
        .await
        .unwrap();
    assert!(exit.cancelled);
    let cancelled = manager
        .start(shell_request(dir.path(), "exec sleep 60"))
        .await
        .unwrap();
    let exit = manager.cancel(cancelled.id).await.unwrap();
    assert!(exit.cancelled);
}

#[tokio::test]
async fn unknown_ids_and_cursor_mismatch_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let unknown = ProcessId::new();
    assert!(matches!(
        manager.wait(unknown).await,
        Err(ProcessError::UnknownProcess(_))
    ));
    let info = manager
        .start(shell_request(dir.path(), "true"))
        .await
        .unwrap();
    assert!(matches!(
        manager
            .read(info.id, OutputCursor::start(unknown), 10)
            .await,
        Err(ProcessError::CursorProcessMismatch)
    ));
}

#[tokio::test]
async fn required_isolation_fails_closed_and_summary_is_truthful() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let mut required = shell_request(dir.path(), "true");
    required.grant.isolation = IsolationRequirement::Required;
    assert!(matches!(
        manager.start(required).await,
        Err(ProcessError::IsolationUnavailable)
    ));

    let info = manager
        .start(shell_request(dir.path(), "true"))
        .await
        .unwrap();
    assert_eq!(info.enforcement.backend, "host");
    assert!(!info
        .enforcement
        .enforced
        .contains(&EnforcementControl::FilesystemIsolation));
    assert!(info
        .enforcement
        .preflight_validated
        .contains(&EnforcementControl::WorkingDirectoryValidated));
}

#[tokio::test]
async fn pty_is_not_silently_downgraded() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let mut request = shell_request(dir.path(), "true");
    request.mode = ProcessMode::Pty;
    assert!(matches!(
        manager.start(request).await,
        Err(ProcessError::UnsupportedCapability("pty"))
    ));
}

#[test]
fn process_listing_and_debug_do_not_reveal_secret_values() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = shell_request(dir.path(), "true");
    request.grant.approved_secret_ids = BTreeSet::from(["service".into()]);
    request.grant.approved_secret_environment = BTreeSet::from(["SERVICE_TOKEN".into()]);
    request.approved_secret_environment.push(SecretEnvironment {
        secret_id: "service".into(),
        name: "SERVICE_TOKEN".into(),
        value: "very-secret".into(),
    });
    assert!(!format!("{request:?}").contains("very-secret"));
}
