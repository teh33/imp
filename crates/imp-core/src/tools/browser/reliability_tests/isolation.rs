use super::*;

#[tokio::test]
async fn tool_error_and_process_exit_invalidate_only_failed_session() {
    let dir = TempDir::new().unwrap();
    let healthy_action = r#"printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"healthy"}],"isError":false}}'"#;
    let failing_action = "exit 17";
    let healthy = BrowserTool::new(config(script(
        dir.path(),
        "healthy",
        &initialized(healthy_action),
    )));
    let failing = BrowserTool::new(config(script(
        dir.path(),
        "failing",
        &initialized(failing_action),
    )));
    let ctx = context(dir.path());
    let healthy_start = start(&healthy, ctx.clone()).await;
    let failing_start = start(&failing, ctx.clone()).await;
    let healthy_id = session_id(&healthy_start).to_string();
    let failing_id = session_id(&failing_start).to_string();

    let failed = failing
        .execute(
            "fail",
            json!({"action": "observe", "session_id": failing_id}),
            ctx.clone(),
        )
        .await
        .unwrap();
    assert!(failed.is_error);
    assert!(text(&failed).contains("session terminated"));
    assert_eq!(failing.sessions.lock().await.session_count_for_test(), 0);

    let survived = healthy
        .execute(
            "healthy",
            json!({"action": "observe", "session_id": healthy_id}),
            ctx,
        )
        .await
        .unwrap();
    assert!(!survived.is_error, "{}", text(&survived));
    assert_eq!(text(&survived), "healthy");
}

#[tokio::test]
async fn cancellation_terminates_session_without_retrying_action() {
    let dir = TempDir::new().unwrap();
    let count = dir.path().join("calls");
    let action = format!("echo call >> '{}'; sleep 2", count.display());
    let tool = BrowserTool::new(config(script(dir.path(), "cancel", &initialized(&action))));
    let ctx = context(dir.path());
    let started = start(&tool, ctx.clone()).await;
    let id = session_id(&started).to_string();
    let cancelled = Arc::clone(&ctx.cancelled);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let output = tool
        .execute(
            "cancel",
            json!({"action": "observe", "session_id": id}),
            ctx,
        )
        .await
        .unwrap();
    assert!(output.is_error);
    assert!(text(&output).contains("cancelled"));
    assert_eq!(tool.sessions.lock().await.session_count_for_test(), 0);
    let calls = fs::read_to_string(count).unwrap();
    assert_eq!(calls.lines().count(), 1);
}

#[tokio::test]
async fn session_limit_and_sequence_are_isolated() {
    let dir = TempDir::new().unwrap();
    let action = r#"printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"ok"}],"isError":false}}'"#;
    let mut browser_config = config(script(dir.path(), "limit", &initialized(action)));
    browser_config.max_sessions = 1;
    let tool = BrowserTool::new(browser_config);
    let ctx = context(dir.path());
    let first = start(&tool, ctx.clone()).await;
    let id = session_id(&first).to_string();
    let second = start(&tool, ctx.clone()).await;
    assert!(second.is_error);
    assert!(text(&second).contains("session limit reached"));

    let called = tool
        .execute("call", json!({"action": "observe", "session_id": id}), ctx)
        .await
        .unwrap();
    assert_eq!(called.details["sequence"], 1);
}

#[tokio::test]
async fn tool_level_error_does_not_retry_or_destroy_healthy_session() {
    let dir = TempDir::new().unwrap();
    let count = dir.path().join("tool-error-calls");
    let action = format!(
        "echo call >> '{}'; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"selector stale\"}}],\"isError\":true}}}}'",
        count.display()
    );
    let tool = BrowserTool::new(config(script(
        dir.path(),
        "tool-error",
        &initialized(&action),
    )));
    let ctx = context(dir.path());
    let started = start(&tool, ctx.clone()).await;
    let id = session_id(&started).to_string();
    let output = tool
        .execute("error", json!({"action": "observe", "session_id": id}), ctx)
        .await
        .unwrap();
    assert!(output.is_error);
    assert_eq!(text(&output), "selector stale");
    assert_eq!(fs::read_to_string(count).unwrap().lines().count(), 1);
    assert_eq!(tool.sessions.lock().await.session_count_for_test(), 1);
}

#[tokio::test]
async fn zero_idle_timeout_prunes_session_before_limit_check() {
    let dir = TempDir::new().unwrap();
    let action = r#"printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"ok"}],"isError":false}}'"#;
    let mut browser_config = config(script(dir.path(), "idle", &initialized(action)));
    browser_config.max_sessions = 1;
    browser_config.idle_timeout_seconds = 0;
    let tool = BrowserTool::new(browser_config);
    let ctx = context(dir.path());
    let first = start(&tool, ctx.clone()).await;
    assert!(!first.is_error, "{}", text(&first));
    let second = start(&tool, ctx).await;
    assert!(!second.is_error, "{}", text(&second));
    assert_eq!(tool.sessions.lock().await.session_count_for_test(), 1);
}
