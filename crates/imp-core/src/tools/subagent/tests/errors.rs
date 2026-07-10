use super::*;

#[tokio::test]
async fn errors_and_policy_denial_are_explicit_without_process_launch() {
    let _env_lock = FAKE_LOOPR_ENV_LOCK.lock().unwrap();
    std::env::remove_var("FAKE_LOOPR_MALFORMED");
    std::env::remove_var("FAKE_LOOPR_NONZERO");
    std::env::remove_var("FAKE_LOOPR_SLEEP");
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("notes")).unwrap();
    fs::write(temp.path().join("notes/input.md"), "context").unwrap();
    fs::create_dir_all(temp.path().join("artifacts")).unwrap();
    let executable = fake_loopr(temp.path());
    let denied = call(
        &SubagentTool::with_executable_and_timeout(executable.clone(), Duration::from_secs(1)),
        test_ctx(temp.path(), RunPolicy::new().allow_write("other/**")),
        json!({"action":"launch", "input":input("child_1")}),
    )
    .await;
    let denied = error_message(denied);
    assert!(denied.to_string().contains("denied"));
    assert!(!temp.path().join("fake-loopr-args.jsonl").exists());
    let missing = call(
        &SubagentTool::with_executable_and_timeout(
            temp.path().join("missing"),
            Duration::from_secs(1),
        ),
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_1")}),
    )
    .await;
    let missing = error_message(missing);
    assert!(missing.to_string().contains("failed to start loopr"));
    std::env::set_var("FAKE_LOOPR_MALFORMED", "1");
    let malformed = call(
        &SubagentTool::with_executable_and_timeout(executable.clone(), Duration::from_secs(1)),
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_1")}),
    )
    .await;
    let malformed = error_message(malformed);
    assert!(malformed.to_string().contains("malformed JSON"));
    std::env::remove_var("FAKE_LOOPR_MALFORMED");
    std::env::set_var("FAKE_LOOPR_NONZERO", "1");
    let nonzero = call(
        &SubagentTool::with_executable_and_timeout(executable.clone(), Duration::from_secs(1)),
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_2")}),
    )
    .await;
    assert!(error_message(nonzero).contains("loopr exited"));
    std::env::remove_var("FAKE_LOOPR_NONZERO");
    let stale = call(
        &SubagentTool::with_executable_and_timeout(executable.clone(), Duration::from_secs(1)),
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"status", "child_run_id":"missing"}),
    )
    .await;
    let stale = error_message(stale);
    assert!(stale.to_string().contains("stale or unknown"));
    std::env::set_var("FAKE_LOOPR_SLEEP", "0.2");
    let timed_out = call(
        &SubagentTool::with_executable_and_timeout(executable, Duration::from_millis(10)),
        test_ctx(temp.path(), RunPolicy::default()),
        json!({"action":"launch", "input":input("child_3")}),
    )
    .await;
    let timed_out = error_message(timed_out);
    assert!(timed_out.to_string().contains("timed out"));
    std::env::remove_var("FAKE_LOOPR_SLEEP");
}
