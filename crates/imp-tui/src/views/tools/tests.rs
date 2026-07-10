use super::*;

fn make_tc(name: &str, args: &str, output: Option<&str>, is_error: bool) -> DisplayToolCall {
    DisplayToolCall {
        id: "test".into(),
        name: name.into(),
        args_summary: args.into(),
        output: output.map(String::from),
        details: serde_json::Value::Null,
        is_error,
        expanded: false,
        notices: Vec::new(),
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    }
}

#[test]
fn tool_header_counts_deduplicated_notices() {
    let mut tc = make_tc("bash", "cargo test", Some("ok"), false);
    tc.add_notice("Trust warning");
    tc.add_notice("Trust warning");
    tc.add_notice("Policy warning");

    let text = tc
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(tc.notices.len(), 2);
    assert!(text.contains("⚠ 2"));
    let compact = tc
        .compact_spans(&Theme::default())
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(compact.contains("⚠ 2"));
}

#[test]
fn compactable_completed_success() {
    let tc = make_tc("read", "file.rs", Some("contents"), false);
    assert!(is_compactable(&tc));
}

#[test]
fn not_compactable_running() {
    let tc = make_tc("read", "file.rs", None, false);
    assert!(!is_compactable(&tc));
}

#[test]
fn not_compactable_error() {
    let tc = make_tc("read", "file.rs", Some("err"), true);
    assert!(!is_compactable(&tc));
}

#[test]
fn not_compactable_expanded() {
    let mut tc = make_tc("read", "file.rs", Some("data"), false);
    tc.expanded = true;
    assert!(!is_compactable(&tc));
}

#[test]
fn short_args_path() {
    assert_eq!(short_args("src/views/tools.rs"), "tools.rs");
}

#[test]
fn short_args_bash() {
    assert_eq!(short_args("check"), "check");
}

#[test]
fn short_args_bash_short() {
    assert_eq!(short_args("run"), "run");
}

#[test]
fn short_args_empty() {
    assert_eq!(short_args(""), "");
}

#[test]
fn short_args_short_text() {
    assert_eq!(short_args("pattern"), "pattern");
}

#[test]
fn abbreviates_user_home_paths() {
    assert_eq!(
        abbreviate_home_path("/Users/test/src/main.rs"),
        "~/src/main.rs"
    );
    assert_eq!(
        abbreviate_home_path("/home/test/src/main.rs"),
        "~/src/main.rs"
    );
}

#[test]
fn tool_headers_include_prototype_icon() {
    let tc = make_tc("prototype", "run", Some("ok"), false);
    let text = tc
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("✓ ⚗ Prototype"));
}

#[test]
fn tool_frame_uses_slow_simple_spinner() {
    assert_eq!(tool_frame(0), "◐");
    assert_eq!(tool_frame(5), "◐");
    assert_eq!(tool_frame(6), "◓");
    assert_eq!(tool_frame(12), "◑");
    assert_eq!(tool_frame(18), "◒");
    assert_eq!(tool_frame(24), "◐");
}

#[test]
fn tool_headers_use_icons_and_status_colors() {
    let tc = make_tc("work", "task", Some("ok"), false);
    let text = tc
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("✓ ▣ Work"));

    let running = make_tc("bash", "cargo test -p imp-tui", None, false);
    let running_text = running
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(running_text.contains("◐ $ cargo test -p imp-tui"));
    assert!(!running_text.contains("Terminal"));
}

#[test]
fn running_tool_header_uses_static_glyph_when_animation_disabled() {
    let running = make_tc("bash", "cargo test -p imp-tui", None, false);
    let text = running
        .header_line_animated(&Theme::default(), 9, AnimationLevel::None)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("• $ cargo test -p imp-tui"));
}

#[test]
fn make_args_summary_hides_bash_command_text() {
    let summary = DisplayToolCall::make_args_summary(
        "bash",
        &serde_json::json!({"command": "cargo test -p imp-tui"}),
    );
    assert_eq!(summary, "cargo test -p imp-tui");
}

#[test]
fn make_args_summary_preserves_full_multiline_bash_command() {
    let command = "cat <<\'EOF\' > /tmp/example\nhello world\nEOF";
    let summary =
        DisplayToolCall::make_args_summary("bash", &serde_json::json!({"command": command}));
    assert_eq!(summary, command);
}

#[test]
fn make_args_summary_abbreviates_scan_directory() {
    let summary = DisplayToolCall::make_args_summary(
        "scan",
        &serde_json::json!({"action": "scan", "directory": "/Users/test/project"}),
    );
    assert_eq!(summary, "~/project");
}

#[test]
fn make_args_summary_shows_scan_search_query() {
    let summary = DisplayToolCall::make_args_summary(
        "scan",
        &serde_json::json!({"action": "search", "query": "ToolExecutionStart"}),
    );
    assert_eq!(summary, "search ToolExecutionStart");
}

#[test]
fn inline_edit_summary_includes_file_and_change_count() {
    let summary = DisplayToolCall::make_args_summary(
        "edit",
        &serde_json::json!({
            "path": "/Users/asher/project/crates/imp-tui/src/views/tool_output.rs",
            "edits": [{"old_text": "old", "new_text": "new"}]
        }),
    );

    assert_eq!(summary, "…/tool_output.rs · 1 change");
}

#[test]
fn inline_edit_summary_uses_path_from_transactional_edit() {
    let summary = DisplayToolCall::make_args_summary(
        "edit",
        &serde_json::json!({
            "edits": [{
                "path": "/Users/asher/project/crates/imp-tui/src/views/tools.rs",
                "old_text": "old",
                "new_text": "new"
            }]
        }),
    );

    assert_eq!(summary, "…/tools.rs · 1 change");
}

#[test]
fn inline_summaries_format_core_tool_arguments() {
    let work = DisplayToolCall::make_args_summary(
        "work",
        &serde_json::json!({
            "action": "create",
            "kind": "task",
            "title": "Improve inline summaries",
            "status": "todo",
            "force": false
        }),
    );
    assert_eq!(work, "create task · todo\n  Improve inline summaries");

    let prototype = DisplayToolCall::make_args_summary(
        "prototype",
        &serde_json::json!({
            "action": "run",
            "question": "Can cards render compact metadata?",
            "language": "python",
            "hypothesis_result": "supported"
        }),
    );
    assert!(prototype.starts_with("run  question Can cards render compact metadat…"));
    assert!(prototype.contains("language python"));
    assert!(prototype.contains("hypothesis_result supported"));

    let workflow_run = DisplayToolCall::make_args_summary(
        "workflow",
        &serde_json::json!({"action": "run", "id": "benchmark-workflow-e2e"}),
    );
    assert_eq!(workflow_run, "run benchmark-workflow-e2e");

    let workflow_update = DisplayToolCall::make_args_summary(
        "workflow",
        &serde_json::json!({
            "action": "update",
            "id": "benchmark-workflow-e2e",
            "path": "steps.context.status",
            "value": "done",
            "reason": "Loaded project instructions and inspected the workflow tool surface"
        }),
    );
    assert_eq!(
        workflow_update,
        "update benchmark-workflow-e2e · steps.context.status → done\n  Loaded project instructions and inspected the workflow tool surface"
    );

    let git = DisplayToolCall::make_args_summary(
        "git",
        &serde_json::json!({"action": "diff", "base": "HEAD~1", "head": "HEAD"}),
    );
    assert_eq!(git, "diff  base HEAD~1  head HEAD");

    let web = DisplayToolCall::make_args_summary(
        "web",
        &serde_json::json!({"action": "search", "query": "imp sidebar design"}),
    );
    assert_eq!(web, "search  query imp sidebar design");
}

#[test]
fn multiline_tool_summary_exposes_detail_line() {
    let tc = DisplayToolCall {
        id: "tc-1".into(),
        name: "work".into(),
        args_summary: "create task · todo\n  Temporary tool smoke test".into(),
        output: Some("ok".into()),
        details: serde_json::Value::Null,
        is_error: false,
        expanded: false,
        notices: Vec::new(),
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    };

    let header = tc
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(header.contains("create task · todo"));
    assert!(!header.contains("\n"));

    let detail = tc.summary_detail_lines(&Theme::default());
    let text = detail[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(text, "  └ Temporary tool smoke test");
}

#[test]
fn inline_header_result_summary_reads_like_status_badge() {
    let tc = make_tc("read", "src/main.rs", Some("one\ntwo"), false);
    let text = tc
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("· 2 lines"));
    assert!(!text.contains("ok"));

    let failed = make_tc("bash", "check", Some("boom"), true);
    let text = failed
        .header_line(&Theme::default())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("failed"));
}

#[test]
fn compact_group_fits_one_line() {
    let tcs = vec![
        make_tc("read", "file.rs", Some("ok"), false),
        make_tc("bash", "$ grep foo .", Some("ok"), false),
    ];
    assert_eq!(compact_group_line_count(&tcs, 80), 1);
}

#[test]
fn compact_group_wraps() {
    let tcs: Vec<_> = (0..10)
        .map(|i| {
            make_tc(
                "read",
                &format!("long/path/to/file_{i}.rs"),
                Some("ok"),
                false,
            )
        })
        .collect();
    let lines = compact_group_line_count(&tcs, 80);
    assert!(lines > 1);
    assert!(lines < 10);
}

#[test]
fn compact_height_mixed() {
    let tcs = vec![
        make_tc("read", "a.rs", Some("ok"), false),
        make_tc("read", "b.rs", Some("ok"), false),
        make_tc("bash", "$ cmd", None, false), // running
        make_tc("read", "c.rs", Some("ok"), false),
    ];
    let h = tool_calls_compact_height(&tcs, 80);
    // First 2 compact (1 line) + 1 running (1 line) + 1 compact (1 line) = 3
    assert_eq!(h, 3);
}

#[test]
fn compact_height_all_compactable() {
    let tcs = vec![
        make_tc("read", "a.rs", Some("ok"), false),
        make_tc("bash", "$ grep foo .", Some("ok"), false),
        make_tc("edit", "b.rs", Some("ok"), false),
    ];
    let h = tool_calls_compact_height(&tcs, 80);
    assert_eq!(h, 1);
}
