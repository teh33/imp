use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use imp_subagent::{Executor, LaunchRequest, Status};
use serde_json::json;

fn fake_child(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("fake-child.py");
    fs::write(
        &path,
        r##"#!/usr/bin/env python3
import json, sys
print(json.dumps({"type":"rpc_ready","protocol":"imp-rpc","version":1}), flush=True)
for line in sys.stdin:
    command = json.loads(line)
    message = {"role":"assistant","content":[{"type":"text","text":command["content"]}]}
    print(json.dumps({"type":"message_end","message":message}), flush=True)
    print(json.dumps({"type":"agent_end","status":{"type":"done"}}), flush=True)
"##,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(&path).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(&path, mode).unwrap();
    }
    path
}

#[test]
fn durable_imp_subagent_worker_uses_only_imp_process_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let executable = std::env::var_os("CARGO_BIN_EXE_imp")
        .map(PathBuf::from)
        .expect("imp test binary");
    let executor = Executor::new(temp.path());
    let record = executor
        .launch(LaunchRequest {
            parent_id: "parent_1".into(),
            child_id: "child_1".into(),
            cwd: temp.path().to_path_buf(),
            prompt: "first".into(),
            executable: fake_child(temp.path()),
            worker_executable: executable,
            child_args: Vec::new(),
            environment: Vec::new(),
            timeout_seconds: None,
            metadata: json!({}),
        })
        .unwrap();
    assert_eq!(record.status, Status::Running);
    let settled = executor
        .wait("parent_1", "child_1", Duration::from_secs(2))
        .unwrap();
    assert_eq!(settled.status, Status::Success);
    assert_eq!(settled.summary.as_deref(), Some("first"));
    let sent = executor
        .send("parent_1", "child_1", "second".into())
        .unwrap();
    assert_eq!(sent.status, Status::Running);
    let settled = executor
        .wait("parent_1", "child_1", Duration::from_secs(2))
        .unwrap();
    assert_eq!(settled.summary.as_deref(), Some("second"));
    assert_eq!(
        executor.cancel("parent_1", "child_1").unwrap().status,
        Status::Success
    );
    let record = executor
        .launch(LaunchRequest {
            parent_id: "parent_1".into(),
            child_id: "child_2".into(),
            cwd: temp.path().to_path_buf(),
            prompt: "third".into(),
            executable: fake_child(temp.path()),
            worker_executable: std::env::var_os("CARGO_BIN_EXE_imp")
                .map(PathBuf::from)
                .unwrap(),
            child_args: Vec::new(),
            environment: Vec::new(),
            timeout_seconds: None,
            metadata: json!({}),
        })
        .unwrap();
    assert_eq!(record.status, Status::Running);
    let cancelled = executor.cancel("parent_1", "child_2").unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(cancelled.artifacts.state.is_file());
}
