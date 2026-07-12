use super::*;
use std::path::PathBuf;
use std::sync::Mutex;

#[test]
fn hook_def_toml_parsing() {
    let toml_str = r#"
[[hooks]]
event = "after_file_write"
match = "*.rs"
action = "shell"
command = "rustfmt {file}"
blocking = true

[[hooks]]
event = "on_context_threshold"
action = "shell"
command = "echo threshold"
threshold = 0.8
"#;

    #[derive(Deserialize)]
    struct Wrapper {
        hooks: Vec<HookDef>,
    }

    let parsed: Wrapper = toml::from_str(toml_str).expect("TOML parsing failed");
    assert_eq!(parsed.hooks.len(), 2);

    let h0 = &parsed.hooks[0];
    assert_eq!(h0.event, "after_file_write");
    assert_eq!(h0.match_pattern.as_deref(), Some("*.rs"));
    assert_eq!(h0.action, "shell");
    assert_eq!(h0.command.as_deref(), Some("rustfmt {file}"));
    assert!(h0.blocking);
    assert!(h0.threshold.is_none());

    let h1 = &parsed.hooks[1];
    assert_eq!(h1.event, "on_context_threshold");
    assert!(h1.match_pattern.is_none());
    assert_eq!(h1.threshold, Some(0.8));
}

#[test]
fn hook_interpolation_file() {
    let event = HookEvent::AfterFileWrite {
        file: Path::new("/tmp/test.rs"),
    };
    let result = interpolate_command("rustfmt {file}", &event);
    assert_eq!(result, "rustfmt /tmp/test.rs");
}

#[test]
fn hook_interpolation_tool_name() {
    let args = serde_json::json!({"path": "/tmp"});
    let event = HookEvent::BeforeToolCall {
        tool_name: "bash",
        args: &args,
    };
    let result = interpolate_command("echo {tool_name}", &event);
    assert_eq!(result, "echo bash");
}

#[test]
fn hook_interpolation_quoted_placeholder() {
    let command_text = "pwd && egrep '(^|/)(README|VISION)\\.md$' && printf '$HOME'";
    let result_msg = ToolResultMessage {
        tool_call_id: "call_quoted".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text { text: "ok".into() }],
        is_error: true,
        details: serde_json::json!({
            "exit_code": 2,
            "command": command_text,
        }),
        timestamp: 0,
    };
    let event = HookEvent::AfterToolCall {
        tool_name: "bash",
        result: &result_msg,
    };

    let interpolated = interpolate_command(
        "hook '{is_error}' '{exit_code}' '{command}' \"{command}\" {command}",
        &event,
    );

    assert_eq!(
        interpolated,
        format!(
            "hook 'true' '2' {} {} {}",
            shell_single_quote(command_text),
            shell_double_quote(command_text),
            command_text
        )
    );
}

#[test]
fn hook_interpolation_ratio() {
    let event = HookEvent::OnContextThreshold { ratio: 0.75 };
    let result = interpolate_command("echo ratio={ratio}", &event);
    assert_eq!(result, "echo ratio=0.75");
}

#[test]
fn hook_event_name_mapping() {
    let path = PathBuf::from("/tmp/test.rs");
    assert_eq!(
        HookEvent::AfterFileWrite { file: &path }.event_name(),
        "after_file_write"
    );
    assert_eq!(HookEvent::BeforeLlmCall.event_name(), "before_llm_call");
    assert_eq!(HookEvent::OnSessionStart.event_name(), "on_session_start");
    assert_eq!(
        HookEvent::OnSessionShutdown.event_name(),
        "on_session_shutdown"
    );
    assert_eq!(
        HookEvent::OnContextThreshold { ratio: 0.5 }.event_name(),
        "on_context_threshold"
    );
}

#[test]
fn hook_matches_event_name() {
    let hook = HookDefinition {
        event: "after_file_write".into(),
        match_pattern: None,
        action: HookAction::Shell {
            command: "echo hi".into(),
        },
        blocking: false,
        threshold: None,
    };
    let path = PathBuf::from("/tmp/test.rs");
    let event = HookEvent::AfterFileWrite { file: &path };
    assert!(matches_event(&hook, &event));

    let wrong_event = HookEvent::BeforeLlmCall;
    assert!(!matches_event(&hook, &wrong_event));
}

#[test]
fn hook_matches_file_glob() {
    let hook = HookDefinition {
        event: "after_file_write".into(),
        match_pattern: Some("*.rs".into()),
        action: HookAction::Shell {
            command: "echo hi".into(),
        },
        blocking: false,
        threshold: None,
    };

    let rs_path = PathBuf::from("/tmp/test.rs");
    let rs_event = HookEvent::AfterFileWrite { file: &rs_path };
    assert!(matches_event(&hook, &rs_event));

    let py_path = PathBuf::from("/tmp/test.py");
    let py_event = HookEvent::AfterFileWrite { file: &py_path };
    assert!(!matches_event(&hook, &py_event));
}

#[test]
fn hook_matches_tool_name() {
    let hook = HookDefinition {
        event: "before_tool_call".into(),
        match_pattern: Some("bash".into()),
        action: HookAction::Shell {
            command: "echo hi".into(),
        },
        blocking: true,
        threshold: None,
    };

    let args = serde_json::json!({});
    let match_event = HookEvent::BeforeToolCall {
        tool_name: "bash",
        args: &args,
    };
    assert!(matches_event(&hook, &match_event));

    let no_match_event = HookEvent::BeforeToolCall {
        tool_name: "read",
        args: &args,
    };
    assert!(!matches_event(&hook, &no_match_event));
}

#[test]
fn hook_threshold_filtering() {
    let hook = HookDefinition {
        event: "on_context_threshold".into(),
        match_pattern: None,
        action: HookAction::Shell {
            command: "echo hi".into(),
        },
        blocking: true,
        threshold: Some(0.8),
    };

    // Below threshold — should not match
    let below = HookEvent::OnContextThreshold { ratio: 0.5 };
    assert!(!matches_event(&hook, &below));

    // At threshold — should match
    let at = HookEvent::OnContextThreshold { ratio: 0.8 };
    assert!(matches_event(&hook, &at));

    // Above threshold — should match
    let above = HookEvent::OnContextThreshold { ratio: 0.95 };
    assert!(matches_event(&hook, &above));
}

#[test]
fn hook_resolve_shell() {
    let def = HookDef {
        event: "after_file_write".into(),
        match_pattern: Some("*.rs".into()),
        action: "shell".into(),
        command: Some("rustfmt {file}".into()),
        blocking: true,
        threshold: None,
    };
    let resolved = resolve_hook_def(def).expect("should resolve");
    assert_eq!(resolved.event, "after_file_write");
    assert!(resolved.blocking);
    assert!(matches!(resolved.action, HookAction::Shell { .. }));
}

#[test]
fn hook_resolve_missing_command_returns_none() {
    let def = HookDef {
        event: "after_file_write".into(),
        match_pattern: None,
        action: "shell".into(),
        command: None,
        blocking: false,
        threshold: None,
    };
    assert!(resolve_hook_def(def).is_none());
}

#[test]
fn hook_resolve_unknown_action_returns_none() {
    let def = HookDef {
        event: "after_file_write".into(),
        match_pattern: None,
        action: "unknown".into(),
        command: Some("echo".into()),
        blocking: false,
        threshold: None,
    };
    assert!(resolve_hook_def(def).is_none());
}

#[tokio::test]
async fn hook_shell_command_timeout_uses_process_runtime_cleanup() {
    let manager = crate::process::ProcessManager::new();
    let error = run_hook_shell_command(
        &manager,
        "trap '' TERM; exec sleep 60",
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert!(error.contains("timed out"));
}

#[tokio::test]
async fn hook_blocking_shell_executes() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "after_file_write".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo hello".into()),
        blocking: true,
        threshold: None,
    }]);

    let path = PathBuf::from("/tmp/test.txt");
    let event = HookEvent::AfterFileWrite { file: &path };
    let results = runner.fire(&event).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].block);
}

#[tokio::test]
async fn hook_non_blocking_fires_and_forgets() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "on_session_start".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo non-blocking".into()),
        blocking: false,
        threshold: None,
    }]);

    let event = HookEvent::OnSessionStart;
    let started = std::time::Instant::now();
    let results = runner.fire(&event).await;
    // Non-blocking hooks don't return results
    assert!(results.is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn hook_non_blocking_failure_is_reported() {
    let mut runner = HookRunner::new();
    let reported = Arc::new(Mutex::new(Vec::new()));
    let reported_clone = Arc::clone(&reported);
    runner.set_background_reporter(Arc::new(move |event| {
        reported_clone.lock().unwrap().push(event);
    }));
    runner.load_from_config(vec![HookDef {
        event: "on_session_start".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("exit 7".into()),
        blocking: false,
        threshold: None,
    }]);

    let event = HookEvent::OnSessionStart;
    let results = runner.fire(&event).await;
    assert!(results.is_empty());

    for _ in 0..20 {
        if !reported.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let reported = reported.lock().unwrap();
    assert_eq!(reported.len(), 1);
    match &reported[0] {
        HookBackgroundEvent::NonBlockingHookFailed { event, command, .. } => {
            assert_eq!(event, "on_session_start");
            assert_eq!(command, "exit 7");
        }
        other => panic!("expected non-blocking hook failure, got {other:?}"),
    }
}

#[tokio::test]
async fn hook_after_tool_call_nonblocking_quoted_command() {
    let temp = tempfile::tempdir().unwrap();
    let output_path = temp.path().join("hook-args.txt");
    let script_path = temp.path().join("capture.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n%s\\n%s\\n' \"$1\" \"$2\" \"$3\" > {}\n",
            output_path.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "after_tool_call".into(),
        match_pattern: Some("bash".into()),
        action: "shell".into(),
        command: Some(format!(
            "{} '{{is_error}}' '{{exit_code}}' '{{command}}'",
            script_path.display()
        )),
        blocking: false,
        threshold: None,
    }]);

    let original_command = "pwd && egrep '(^|/)(README|VISION)\\.md$' | sort && printf '$HOME'";
    let result_msg = ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        content: vec![ContentBlock::Text {
            text: "failed".into(),
        }],
        is_error: true,
        details: serde_json::json!({
            "exit_code": 2,
            "command": original_command,
        }),
        timestamp: 0,
    };
    let event = HookEvent::AfterToolCall {
        tool_name: "bash",
        result: &result_msg,
    };

    let results = runner.fire(&event).await;
    assert!(results.is_empty());

    for _ in 0..200 {
        if output_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let captured = std::fs::read_to_string(&output_path).unwrap();
    let mut lines = captured.lines();
    assert_eq!(lines.next(), Some("true"));
    assert_eq!(lines.next(), Some("2"));
    assert_eq!(lines.next(), Some(original_command));
}

#[test]
fn report_non_blocking_hook_outcome_maps_join_failure_to_panic_event() {
    let reported = Arc::new(Mutex::new(Vec::new()));
    let reported_clone = Arc::clone(&reported);
    let reporter: Arc<dyn Fn(HookBackgroundEvent) + Send + Sync> = Arc::new(move |event| {
        reported_clone.lock().unwrap().push(event);
    });

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let join_error = runtime.block_on(async {
        tokio::spawn(async move {
            panic!("intentional join failure for reporting test");
        })
        .await
        .unwrap_err()
    });
    drop(runtime);

    let _ = std::panic::take_hook();
    std::panic::set_hook(previous_hook);

    report_non_blocking_hook_outcome(
        Err(join_error),
        "on_session_start".into(),
        "test command".into(),
        reporter,
    );

    let reported = reported.lock().unwrap();
    assert_eq!(reported.len(), 1);
    match &reported[0] {
        HookBackgroundEvent::NonBlockingHookPanicked {
            event,
            command,
            error,
        } => {
            assert_eq!(event, "on_session_start");
            assert_eq!(command, "test command");
            assert!(error.contains("panic") || error.contains("cancelled"));
        }
        other => panic!("expected non-blocking hook panic, got {other:?}"),
    }
}

#[tokio::test]
async fn hook_before_tool_call_blocks() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "before_tool_call".into(),
        match_pattern: Some("bash".into()),
        action: "shell".into(),
        command: Some("exit 1".into()),
        blocking: true,
        threshold: None,
    }]);

    let args = serde_json::json!({"command": "rm -rf /"});
    let event = HookEvent::BeforeToolCall {
        tool_name: "bash",
        args: &args,
    };
    let results = runner.fire(&event).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].block);
}

#[tokio::test]
async fn hook_before_tool_call_allows() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "before_tool_call".into(),
        match_pattern: Some("read".into()),
        action: "shell".into(),
        command: Some("exit 0".into()),
        blocking: true,
        threshold: None,
    }]);

    let args = serde_json::json!({});
    let event = HookEvent::BeforeToolCall {
        tool_name: "read",
        args: &args,
    };
    let results = runner.fire(&event).await;
    assert_eq!(results.len(), 1);
    assert!(!results[0].block);
}

#[tokio::test]
async fn hook_after_tool_call_modifies_result() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "after_tool_call".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo modified output".into()),
        blocking: true,
        threshold: None,
    }]);

    let result_msg = ToolResultMessage {
        tool_call_id: "call_1".into(),
        tool_name: "test".into(),
        content: vec![ContentBlock::Text {
            text: "original".into(),
        }],
        is_error: false,
        details: serde_json::Value::Null,
        timestamp: 0,
    };
    let event = HookEvent::AfterToolCall {
        tool_name: "test",
        result: &result_msg,
    };
    let results = runner.fire(&event).await;
    assert_eq!(results.len(), 1);
    let modified = results[0]
        .modified_content
        .as_ref()
        .expect("should have modified content");
    assert_eq!(modified.len(), 1);
    if let ContentBlock::Text { text } = &modified[0] {
        assert_eq!(text, "modified output");
    } else {
        panic!("expected Text content block");
    }
}

#[tokio::test]
async fn hook_context_threshold_fires_at_correct_ratio() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "on_context_threshold".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo threshold hit at {ratio}".into()),
        blocking: true,
        threshold: Some(0.8),
    }]);

    // Below threshold — no results
    let below = HookEvent::OnContextThreshold { ratio: 0.5 };
    let results = runner.fire(&below).await;
    assert!(results.is_empty());

    // At threshold — should fire
    let at = HookEvent::OnContextThreshold { ratio: 0.8 };
    let results = runner.fire(&at).await;
    assert_eq!(results.len(), 1);

    // Above threshold — should fire
    let above = HookEvent::OnContextThreshold { ratio: 0.95 };
    let results = runner.fire(&above).await;
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn hook_execution_order_toml_first_then_programmatic() {
    use std::sync::Mutex;

    let order = Arc::new(Mutex::new(Vec::new()));

    let mut runner = HookRunner::new();

    // TOML hook
    runner.load_from_config(vec![HookDef {
        event: "on_session_start".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo toml".into()),
        blocking: true,
        threshold: None,
    }]);

    // Programmatic hook
    let order_clone = Arc::clone(&order);
    runner.register_callback(
        "on_session_start",
        Arc::new(move |_event| {
            order_clone.lock().unwrap().push("programmatic");
            HookResult::default()
        }),
    );

    let event = HookEvent::OnSessionStart;
    let results = runner.fire(&event).await;

    // Both should fire
    assert_eq!(results.len(), 2);

    // Programmatic should have recorded its execution
    let recorded = order.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0], "programmatic");
}

#[tokio::test]
async fn hook_callback_blocks_tool_call() {
    let mut runner = HookRunner::new();
    runner.register_callback(
        "before_tool_call",
        Arc::new(|_event| HookResult {
            block: true,
            reason: Some("blocked by callback".into()),
            modified_content: None,
        }),
    );

    let args = serde_json::json!({});
    let event = HookEvent::BeforeToolCall {
        tool_name: "bash",
        args: &args,
    };
    let results = runner.fire(&event).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].block);
    assert_eq!(results[0].reason.as_deref(), Some("blocked by callback"));
}

#[tokio::test]
async fn hook_shell_interpolation_in_execution() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let marker_file = tempfile::NamedTempFile::new().unwrap();
    let marker_path = marker_file.path().to_string_lossy().to_string();

    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "after_file_write".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some(format!("echo {{file}} > {marker_path}")),
        blocking: true,
        threshold: None,
    }]);

    let event = HookEvent::AfterFileWrite { file: &tmp_path };
    runner.fire(&event).await;

    // Verify the marker file contains the interpolated path
    let content = std::fs::read_to_string(&marker_path).unwrap();
    assert!(
        content.contains(&tmp_path.to_string_lossy().to_string()),
        "Expected marker to contain file path, got: {content}"
    );
}

#[test]
fn hook_runner_load_from_config_resolves_all() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![
        HookDef {
            event: "after_file_write".into(),
            match_pattern: Some("*.rs".into()),
            action: "shell".into(),
            command: Some("rustfmt {file}".into()),
            blocking: true,
            threshold: None,
        },
        HookDef {
            event: "before_tool_call".into(),
            match_pattern: Some("bash".into()),
            action: "shell".into(),
            command: Some("echo checking".into()),
            blocking: true,
            threshold: None,
        },
    ]);
    assert_eq!(runner.toml_hooks.len(), 2);
}

#[tokio::test]
async fn hook_unmatched_event_returns_empty() {
    let mut runner = HookRunner::new();
    runner.load_from_config(vec![HookDef {
        event: "on_session_start".into(),
        match_pattern: None,
        action: "shell".into(),
        command: Some("echo hi".into()),
        blocking: true,
        threshold: None,
    }]);

    // Fire a different event
    let event = HookEvent::BeforeLlmCall;
    let results = runner.fire(&event).await;
    assert!(results.is_empty());
}
