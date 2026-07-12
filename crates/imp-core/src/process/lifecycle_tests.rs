#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

fn request(cwd: &Path, script: &str) -> ProcessRequest {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    ProcessRequest {
        command: CommandSpec::new("sh").with_arguments(["-c".into(), script.into()]),
        cwd: cwd.to_path_buf(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(Duration::from_secs(10)),
        output_retention_bytes: 64 * 1024,
        grant,
    }
}

async fn read_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "PID file was not created");
        tokio::task::yield_now().await;
    }
}

async fn wait_until_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(pid) {
        assert!(Instant::now() < deadline, "process {pid} survived cleanup");
        tokio::task::yield_now().await;
    }
}

fn process_exists(raw_pid: i32) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(raw_pid) else {
        return false;
    };
    rustix::process::kill_process(pid, rustix::process::Signal::CONT).is_ok()
}

#[tokio::test]
async fn stop_forces_kill_after_grace_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let info = manager
        .start(request(
            dir.path(),
            "trap '' TERM; printf ready; while :; do sleep 1; done",
        ))
        .await
        .unwrap();
    let output = manager
        .observe(
            info.id,
            OutputCursor::start(info.id),
            Duration::from_secs(1),
            1024,
        )
        .await
        .unwrap();
    assert!(output
        .chunks
        .iter()
        .any(|chunk| chunk.text.contains("ready")));
    let exit = manager
        .stop(
            info.id,
            StopOptions {
                grace: Duration::from_millis(20),
            },
        )
        .await
        .unwrap();
    assert!(exit.cancelled);
    assert_eq!(exit.signal, Some(9));
}

#[tokio::test]
async fn stop_cleans_up_spawned_grandchild() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = format!(
        "sleep 60 & child=$!; printf '%s' \"$child\" > {}; wait",
        pid_file.display()
    );
    let manager = ProcessManager::new();
    let info = manager.start(request(dir.path(), &script)).await.unwrap();
    let grandchild = read_pid(&pid_file).await;
    manager.stop(info.id, StopOptions::default()).await.unwrap();
    wait_until_gone(grandchild).await;
}

#[tokio::test]
async fn manager_drop_kills_owned_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let script = format!("printf '%s' \"$$\" > {}; exec sleep 60", pid_file.display());
    let manager = ProcessManager::new();
    manager.start(request(dir.path(), &script)).await.unwrap();
    let child = read_pid(&pid_file).await;
    drop(manager);
    wait_until_gone(child).await;
}

#[tokio::test]
async fn retained_output_redacts_secret_values() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new();
    let mut request = request(dir.path(), "printf '%s' \"$SERVICE_TOKEN\"");
    request.grant.approved_secret_ids = BTreeSet::from(["service".into()]);
    request.grant.approved_secret_environment = BTreeSet::from(["SERVICE_TOKEN".into()]);
    request.approved_secret_environment.push(SecretEnvironment {
        secret_id: "service".into(),
        name: "SERVICE_TOKEN".into(),
        value: "private-token".into(),
    });
    let info = manager.start(request).await.unwrap();
    manager.wait(info.id).await.unwrap();
    let output = manager
        .read(info.id, OutputCursor::start(info.id), 1024)
        .await
        .unwrap();
    let text = output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert!(text.contains("[REDACTED_SECRET]"));
    assert!(!text.contains("private-token"));
}
