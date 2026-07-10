use super::*;

#[tokio::test]
#[ignore = "requires LOOPR_BIN pointing to a real loopr binary"]
async fn real_loopr_process_boundary_smoke() {
    let loopr = PathBuf::from(std::env::var_os("LOOPR_BIN").expect("set LOOPR_BIN"));
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("notes")).unwrap();
    fs::write(temp.path().join("notes/input.md"), "context").unwrap();
    fs::create_dir_all(temp.path().join("artifacts")).unwrap();
    let agent = temp.path().join("fake-imp.py");
    fs::write(&agent, r##"#!/usr/bin/env python3
import json, sys
print(json.dumps({"type":"rpc_ready","protocol":"imp-rpc","version":1,"capabilities":["durable_sessions","prompt","followup","steer","cancel"]}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if request.get("type") == "prompt":
        message = {"role":"assistant","content":[{"type":"text","text":"fake child complete"}]}
        print(json.dumps({"type":"message_end","message":message}), flush=True)
        print(json.dumps({"type":"agent_end","status":{"type":"done","reason":"work_completed"}}), flush=True)
"##).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(&agent).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(&agent, mode).unwrap();
    }
    fs::create_dir_all(temp.path().join(".loopr")).unwrap();
    fs::write(
        temp.path().join(".loopr/config.toml"),
        format!("default_agent = \"main\"\ndefault_backend = \"imp-rpc\"\n\n[agents.main]\ncommand = \"{}\"\nbackend = \"imp-rpc\"\n", agent.display()),
    ).unwrap();
    let tool = SubagentTool::with_executable_and_timeout(loopr, Duration::from_secs(5));
    for child in ["child_1", "child_2"] {
        call(
            &tool,
            test_ctx(temp.path(), RunPolicy::default()),
            json!({"action":"launch", "input":input(child)}),
        )
        .await
        .unwrap();
    }
    let waited = call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"wait", "child_run_id":"child_1", "timeout_seconds":2}),
    )
    .await
    .unwrap();
    assert_eq!(waited.details["status"], "success");
    let sent = call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"send", "child_run_id":"child_1", "message":"follow-up"}),
    )
    .await
    .unwrap();
    assert_eq!(sent.details["status"], "running");
    let reloaded = SubagentTool::with_executable_and_timeout(
        PathBuf::from(std::env::var_os("LOOPR_BIN").unwrap()),
        Duration::from_secs(5),
    );
    let cancelled = call(
        &reloaded,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"cancel", "child_run_id":"child_2"}),
    )
    .await
    .unwrap();
    assert_eq!(cancelled.details["status"], "cancelled");
}
