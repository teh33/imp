use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::agent::{
    ParentRunId, SubagentContext, SubagentMergePolicy, SubagentResourceLimits, SubagentRole,
};
use crate::config::{AgentMode, Config};
use crate::policy::RunPolicy;
use crate::tools::{AnchorStore, CheckpointState, FileCache, FileTracker, ToolUpdate};
use crate::trust::Provenance;
use crate::ui::NullInterface;
use crate::workflow_review::TurnWorkflowReviewAccumulator;

static FAKE_LOOPR_ENV_LOCK: Mutex<()> = Mutex::new(());

fn test_ctx(dir: &Path, run_policy: RunPolicy) -> ToolContext {
    let (update_tx, _) = tokio::sync::mpsc::channel::<ToolUpdate>(8);
    let (command_tx, _) = tokio::sync::mpsc::channel(8);
    ToolContext {
        cwd: dir.to_path_buf(),
        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        update_tx,
        command_tx,
        ui: Arc::new(NullInterface),
        file_cache: Arc::new(FileCache::new()),
        checkpoint_state: Arc::new(CheckpointState::new()),
        file_tracker: Arc::new(std::sync::Mutex::new(FileTracker::new())),
        anchor_store: Arc::new(AnchorStore::new()),
        lua_tool_loader: None,
        mode: AgentMode::Full,
        read_max_lines: 500,
        turn_workflow_review: Arc::new(std::sync::Mutex::new(
            TurnWorkflowReviewAccumulator::default(),
        )),
        config: Arc::new(Config::default()),
        run_policy,
        supporting_provenance: Vec::<Provenance>::new(),
    }
}

fn input(child: &str) -> SubagentInput {
    SubagentInput {
        parent_run_id: ParentRunId::new("parent_1"),
        child_run_id: SubagentRunId::new(child),
        role: SubagentRole::Verifier,
        objective: "Verify safely; quotes ' and $() remain data".into(),
        context: SubagentContext::default(),
        resource_limits: SubagentResourceLimits {
            timeout_seconds: Some(10),
            allowed_paths: vec![PathBuf::from("notes/input.md")],
            writable_paths: vec![PathBuf::from("artifacts/out.md")],
            ..SubagentResourceLimits::default()
        },
        merge_policy: SubagentMergePolicy::Verify,
        output_contract: Some("Report evidence.".into()),
    }
}

fn fake_loopr(dir: &Path) -> PathBuf {
    let path = dir.join("fake-loopr.py");
    fs::write(&path, r##"#!/usr/bin/env python3
import json, os, pathlib, sys, time
args = sys.argv[1:]
with open("fake-loopr-args.jsonl", "a") as log:
    log.write(json.dumps(args) + "\n")
if os.environ.get("FAKE_LOOPR_SLEEP"):
    time.sleep(float(os.environ["FAKE_LOOPR_SLEEP"]))
if os.environ.get("FAKE_LOOPR_NONZERO"):
    print("backend failed", file=sys.stderr); sys.exit(9)
if os.environ.get("FAKE_LOOPR_MALFORMED"):
    print("not-json"); sys.exit(0)
state = pathlib.Path("fake-loopr-state.json")
try: records = json.loads(state.read_text())
except FileNotFoundError: records = {}
def emit(value): print(json.dumps(value)); state.write_text(json.dumps(records))
if args[:2] == ["start", "--json"]:
    emit({"id":"loopr_run_1"})
elif args[:2] == ["thread", "spawn"]:
    child = "thr_" + str(len(records) + 1)
    records[child] = {"status":"running", "session_id":"ses_" + child, "turn_id":"turn_1"}
    emit({"id":child, **records[child], "result_path":"/tmp/result.md", "status_path":"/tmp/status.json", "stdout_path":"/tmp/out.log", "stderr_path":"/tmp/err.log"})
else:
    action, child = args[1], args[2]
    record = records[child]
    if action == "send":
        record["status"] = "running"; record["turn_id"] = "turn_2"
        emit({"thread":child, "session":record["session_id"], "turn":"turn_2", "sent":True})
    elif action == "stop":
        record["status"] = "stopped"; emit({"thread":child, "status":"stopped", "stopped":True})
    else:
        child_status = os.environ.get("FAKE_LOOPR_CHILD_STATUS")
        status = record["status"]
        if action == "wait" and status == "running": status = "joined"
        payload = {"thread":child, "ready":status == "joined", "status":status,
                   "result":"result text" if status == "joined" else None}
        if child_status:
            payload["child_status"] = {"status":child_status, "summary":"child summary",
              "files_changed":["artifacts/out.md"], "files_inspected":["notes/input.md"],
              "checks_run":["cargo test"], "blockers":["needs review"], "recommended_next_prompt":"review"}
        emit(payload)
"##).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = fs::metadata(&path).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(&path, mode).unwrap();
    }
    path
}

async fn call(
    tool: &SubagentTool,
    ctx: ToolContext,
    params: serde_json::Value,
) -> Result<ToolOutput> {
    tool.execute("call", params, ctx).await
}

fn error_message(result: Result<ToolOutput>) -> String {
    match result {
        Err(error) => error.to_string(),
        Ok(_) => panic!("expected subagent tool error"),
    }
}

#[tokio::test]
async fn launch_persists_mapping_and_preserves_unsafe_prompt_as_one_argument() {
    let _env_lock = FAKE_LOOPR_ENV_LOCK.lock().unwrap();
    std::env::remove_var("FAKE_LOOPR_MALFORMED");
    std::env::remove_var("FAKE_LOOPR_NONZERO");
    std::env::remove_var("FAKE_LOOPR_SLEEP");
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("notes")).unwrap();
    fs::write(temp.path().join("notes/input.md"), "context").unwrap();
    fs::create_dir_all(temp.path().join("artifacts")).unwrap();
    let tool =
        SubagentTool::with_executable_and_timeout(fake_loopr(temp.path()), Duration::from_secs(1));
    let result = call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_1")}),
    )
    .await
    .unwrap();
    assert_eq!(result.details["status"], "running");
    assert_eq!(
        result.details["result"]["unsupported_resource_limits"],
        json!(["timeout_seconds=10"])
    );
    assert_eq!(
        result.details["result"]["mapping"]["loopr_thread_id"],
        "thr_1"
    );
    assert!(temp
        .path()
        .join(".imp/runs/parent_1/subagents/child_1.json")
        .is_file());
    let calls = fs::read_to_string(temp.path().join("fake-loopr-args.jsonl")).unwrap();
    assert!(calls.contains("Verify safely; quotes ' and $() remain data"));
    assert_eq!(calls.lines().count(), 2);
}

#[tokio::test]
async fn lifecycle_maps_terminal_outcomes_routes_send_and_reloads_mapping() {
    let _env_lock = FAKE_LOOPR_ENV_LOCK.lock().unwrap();
    std::env::remove_var("FAKE_LOOPR_MALFORMED");
    std::env::remove_var("FAKE_LOOPR_NONZERO");
    std::env::remove_var("FAKE_LOOPR_SLEEP");
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("notes")).unwrap();
    fs::write(temp.path().join("notes/input.md"), "context").unwrap();
    fs::create_dir_all(temp.path().join("artifacts")).unwrap();
    let executable = fake_loopr(temp.path());
    let tool =
        SubagentTool::with_executable_and_timeout(executable.clone(), Duration::from_secs(1));
    call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_1")}),
    )
    .await
    .unwrap();
    call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_2")}),
    )
    .await
    .unwrap();
    let status = call(
        &tool,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"status", "child_run_id":"child_1"}),
    )
    .await
    .unwrap();
    assert_eq!(status.details["status"], "running");
    assert!(temp
        .path()
        .join(".imp/runs/parent_1/subagents/child_1.json")
        .is_file());
    assert!(temp
        .path()
        .join(".imp/runs/parent_1/subagents/child_2.json")
        .is_file());
    let calls = fs::read_to_string(temp.path().join("fake-loopr-args.jsonl")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.contains("\"start\""))
            .count(),
        1
    );
    let reloaded = SubagentTool::with_executable_and_timeout(executable, Duration::from_secs(1));
    let sent = call(
        &reloaded,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"send", "child_run_id":"child_1", "message":"follow up"}),
    )
    .await
    .unwrap();
    assert_eq!(sent.details["result"]["sent"]["thread"], "thr_1");
    for (child, expected) in [
        ("done", "success"),
        ("blocked", "blocked"),
        ("needs_input", "incomplete"),
        ("failed", "failed"),
        ("cancelled", "cancelled"),
    ] {
        std::env::set_var("FAKE_LOOPR_CHILD_STATUS", child);
        let waited = call(
            &reloaded,
            test_ctx(temp.path(), RunPolicy::default()),
            json!({"action":"wait", "child_run_id":"child_1", "timeout_seconds":0}),
        )
        .await
        .unwrap();
        assert_eq!(waited.details["status"], expected);
        assert_eq!(waited.details["result"]["outcome"]["status"], expected);
        if expected == "success" {
            let mapping = fs::read_to_string(
                temp.path()
                    .join(".imp/runs/parent_1/subagents/child_1.json"),
            )
            .unwrap();
            assert!(mapping.contains("result text"));
        }
    }
    std::env::remove_var("FAKE_LOOPR_CHILD_STATUS");
    let cancelled = call(
        &reloaded,
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"cancel", "child_run_id":"child_2"}),
    )
    .await
    .unwrap();
    assert_eq!(cancelled.details["status"], "cancelled");
}

mod errors;
mod smoke;
