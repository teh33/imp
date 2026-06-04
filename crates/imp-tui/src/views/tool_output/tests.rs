use super::*;
use serde_json::json;

fn make_tc(name: &str, output: Option<&str>) -> DisplayToolCall {
    DisplayToolCall {
        id: format!("tc-{name}"),
        name: name.into(),
        args_summary: "src/main.rs".into(),
        output: output.map(str::to_string),
        details: serde_json::Value::Null,
        is_error: false,
        expanded: true,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    }
}

fn plain_lines(lines: Vec<Line<'static>>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect()
}

#[test]
fn prototype_sidebar_card_shows_experiment_metadata() {
    let mut tc = make_tc(
        "prototype",
        Some("Prototype answered question\nOutput:\nok"),
    );
    tc.details = json!({
        "action": "run",
        "question": "Can the parser handle nested cards?",
        "language": "python",
        "exit_code": 0,
        "outcome": "observed",
        "hypothesis_result": "supported",
        "sandbox": "/tmp/prototype"
    });

    let plain = plain_lines(styled_sidebar_tool_output_lines(
        &tc,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));

    assert_eq!(plain[0], "⚗Prototype · run");
    assert!(plain.iter().any(|line| line.contains("language: python")));
    assert!(plain
        .iter()
        .any(|line| line.contains("hypothesis: supported")));
    assert!(plain.iter().any(|line| line == "ok"));
}

#[test]
fn read_write_edit_sidebar_cards_show_file_metadata() {
    let mut read = make_tc("read", Some("fn main() {}"));
    read.details = json!({"path": "src/main.rs", "lines": 1});
    let read_plain = plain_lines(styled_sidebar_tool_output_lines(
        &read,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(read_plain[0], "◧Read");
    assert!(read_plain
        .iter()
        .any(|line| line.contains("path: src/main.rs")));
    assert!(read_plain.iter().any(|line| line.contains("fn main")));

    let mut write = make_tc("write", Some("src/main.rs: 12 bytes written"));
    write.details = json!({
        "path": "src/main.rs",
        "mode": "overwrite",
        "summary": "src/main.rs: 12 bytes written",
        "display_content": "fn main() {}"
    });
    let write_plain = plain_lines(styled_sidebar_tool_output_lines(
        &write,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(write_plain[0], "✎Write");
    assert!(write_plain
        .iter()
        .any(|line| line.contains("mode: overwrite")));

    let mut edit = make_tc("edit", Some("@@ -1 +1 @@\n-old\n+new"));
    edit.details = json!({
        "path": "src/main.rs",
        "edits": [{"old_text": "old", "new_text": "new"}]
    });
    let edit_plain = plain_lines(styled_sidebar_tool_output_lines(
        &edit,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(edit_plain[0], "◇Edit");
    assert!(edit_plain
        .iter()
        .any(|line| line.contains("edits: 1 change")));
    assert!(edit_plain.iter().any(|line| line.contains("+new")));
}

#[test]
fn shell_sidebar_card_shows_command_and_output() {
    let mut tc = make_tc("bash", Some("PASS tests\nwarning: slow"));
    tc.details = json!({"command": "cargo test -p imp-tui", "workdir": "/repo"});

    let plain = plain_lines(styled_sidebar_tool_output_lines(
        &tc,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));

    assert_eq!(plain[0], "$Terminal");
    assert!(plain
        .iter()
        .any(|line| line.contains("command: cargo test")));
    assert!(plain.iter().any(|line| line.contains("workdir: /repo")));
    assert!(plain.iter().any(|line| line == "PASS tests"));
}

#[test]
fn git_scan_and_web_sidebar_cards_show_key_metadata() {
    let mut git = make_tc("git", Some("On branch nightly"));
    git.details = json!({"action": "status", "base": "HEAD~1", "head": "HEAD"});
    let git_plain = plain_lines(styled_sidebar_tool_output_lines(
        &git,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(git_plain[0], "◆Git · status");
    assert!(git_plain.iter().any(|line| line.contains("base: HEAD~1")));

    let mut scan = make_tc("scan", Some("Functions: 2"));
    scan.details = json!({"action": "search", "query": "WorkTool", "directory": "crates"});
    let scan_plain = plain_lines(styled_sidebar_tool_output_lines(
        &scan,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(scan_plain[0], "⌕Scan · search");
    assert!(scan_plain
        .iter()
        .any(|line| line.contains("query: WorkTool")));

    let mut web = make_tc("web", Some("https://example.com"));
    web.details = json!({"action": "read", "url": "https://example.com"});
    let web_plain = plain_lines(styled_sidebar_tool_output_lines(
        &web,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));
    assert_eq!(web_plain[0], "◎Web · read");
    assert!(web_plain
        .iter()
        .any(|line| line.contains("url: https://example.com")));
}

#[test]
fn workflow_sidebar_card_shows_metadata_and_output() {
    let mut tc = make_tc(
        "workflow",
        Some("Updated workflow `benchmark-workflow-e2e`: steps.context.status = done"),
    );
    tc.details = json!({
        "action": "update",
        "id": "benchmark-workflow-e2e",
        "path": "steps.context.status",
        "value": "done",
        "reason": "Loaded project instructions"
    });

    let plain = plain_lines(styled_sidebar_tool_output_lines(
        &tc,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));

    assert_eq!(plain[0], "⚑Workflow · update");
    assert!(plain
        .iter()
        .any(|line| line.contains("workflow: benchmark-workflow-e2e")));
    assert!(plain
        .iter()
        .any(|line| line.contains("path: steps.context.status")));
    assert!(plain.iter().any(|line| line.contains("value: done")));
    assert!(plain.iter().any(|line| line.contains("Updated workflow")));
}

#[test]
fn workflow_run_agent_action_renders_progress_card_inline_and_sidebar() {
    let mut tc = make_tc(
        "workflow",
        Some("Workflow needs agent action: inspect [context]"),
    );
    tc.details = json!({
        "action": "run",
        "result": {
            "id": "tui-workflow-run-progress",
            "status": "planned",
            "next_action": {
                "kind": "agent_action",
                "step": "inspect",
                "step_kind": "context",
                "contract": {
                    "role": "coder",
                    "objective": "Inspect workflow progress rendering.",
                    "instructions": ["Read app.rs", "Write view model notes"],
                    "write_scope": [".imp/workflows/tui-workflow-run-progress/artifacts/view-model.md"],
                    "completion_checks": ["current_tui_workflow_surface_inspected"],
                    "completion_artifacts": [".imp/workflows/tui-workflow-run-progress/artifacts/view-model.md"]
                }
            }
        }
    });

    for lines in [
        styled_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false),
        styled_sidebar_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false),
    ] {
        let plain = plain_lines(lines);
        assert_eq!(plain[0], "⚑Workflow · run");
        assert!(plain
            .iter()
            .any(|line| line.contains("workflow: tui-workflow-run-progress")));
        assert!(plain.iter().any(|line| line.contains("status: planned")));
        assert!(plain.iter().any(|line| line.contains("step: inspect")));
        assert!(plain.iter().any(|line| line.contains("role: coder")));
        assert!(plain
            .iter()
            .any(|line| line.contains("objective: Inspect workflow progress rendering.")));
        assert!(plain.iter().any(|line| line.contains("instructions:")));
        assert!(plain.iter().any(|line| line.contains("Read app.rs")));
        assert!(plain.iter().any(|line| line.contains("allowed writes:")));
        assert!(plain.iter().any(|line| line.contains("completion checks:")));
        assert!(!plain.iter().any(|line| line.contains("{\"")), "{plain:#?}");
    }
}

#[test]
fn workflow_run_command_orchestration_renders_progress_card() {
    let mut tc = make_tc(
        "workflow",
        Some("Workflow `fixture` orchestrated 2 step(s)."),
    );
    tc.details = json!({
        "action": "run",
        "result": {
            "id": "fixture",
            "status": "done",
            "next_action": {
                "kind": "orchestrated_command_checks",
                "steps": [
                    {"step": "build", "step_status": "done", "checks": [{"check": "build_ok"}]},
                    {"step": "verify", "step_status": "done", "checks": [{"check": "tests_ok"}]}
                ],
                "reconciled": ["status"]
            }
        }
    });

    let plain = plain_lines(styled_tool_output_lines(
        &tc,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));

    assert_eq!(plain[0], "⚑Workflow · run");
    assert!(plain.iter().any(|line| line.contains("status: done")));
    assert!(plain
        .iter()
        .any(|line| line.contains("ran: 2 step(s), 2 check(s)")));
    assert!(plain.iter().any(|line| line.contains("✓ build — done")));
    assert!(plain.iter().any(|line| line.contains("✓ verify — done")));
}

#[test]
fn workflow_run_missing_contract_renders_blocked_card() {
    let mut tc = make_tc(
        "workflow",
        Some("Workflow blocked: missing action contract for step inspect [context]."),
    );
    tc.details = json!({
        "action": "run",
        "result": {
            "id": "fixture",
            "status": "active",
            "next_action": {
                "kind": "missing_action_contract",
                "step": "inspect",
                "step_kind": "context",
                "reason": "Add command checks, a child workflow, worker, or action contract."
            }
        }
    });

    let plain = plain_lines(styled_sidebar_tool_output_lines(
        &tc,
        &Highlighter::new(),
        &Theme::default(),
        false,
    ));

    assert_eq!(plain[0], "⚑Workflow · run");
    assert!(plain.iter().any(|line| line.contains("step: inspect")));
    assert!(plain
        .iter()
        .any(|line| line.contains("blocked: Add command checks")));
}

#[test]
fn work_output_renders_tasks_as_styled_cards() {
    let mut tc = make_tc("work", Some("1 global project task(s)"));
    tc.details = json!({
        "action": "list",
        "kind": "task",
        "items": [{
            "id": "T-improve-work-sidebar",
            "title": "Improve work sidebar",
            "status": "ready",
            "acceptance": ["shows full task detail"],
            "checks": [{"command": "cargo test -p imp-tui work_output"}]
        }],
        "policy": {"decision": "allowed", "tool_name": "work"}
    });

    let lines = styled_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false);
    let plain = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(plain.iter().any(|line| line.contains("▣Work · list")));
    assert!(plain
        .iter()
        .any(|line| line.contains("T-improve-work-sidebar")));
    assert!(plain
        .iter()
        .any(|line| line.contains("shows full task detail")));
    assert!(plain.iter().any(|line| line.contains("policy")));
    assert!(!plain.iter().any(|line| line.contains("{\"")));
}

#[test]
fn read_output_prefers_live_streaming_transcript_while_running() {
    let mut tc = make_tc("bash", None);
    tc.streaming_output = "first\nsecond".into();

    let lines = styled_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false);
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();

    assert_eq!(plain, vec!["first".to_string(), "second".to_string()]);
}

#[test]
fn write_output_uses_structured_display_content() {
    let mut tc = make_tc("write", Some("summary only"));
    tc.details = json!({
        "summary": "src/main.rs: 42 bytes created",
        "display_content": "fn main() {}",
        "path": "src/main.rs"
    });

    let lines = styled_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false);
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();

    assert_eq!(plain[0], "src/main.rs: 42 bytes created");
    assert!(plain.iter().any(|line| line.contains("fn main()")));
}

#[test]
fn git_diff_sidebar_output_adds_compact_line_number_gutter() {
    let mut tc = make_tc(
        "git",
        Some(
            "diff --git a/src/main.rs b/src/main.rs\n@@ -10,3 +10,4 @@ fn main() {\n context\n-old\n+new\n+extra\n",
        ),
    );
    tc.details = json!({ "action": "diff" });

    let lines =
        styled_sidebar_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false);
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();

    assert_eq!(plain[0], "diff --git a/src/main.rs b/src/main.rs");
    assert_eq!(plain[1], "@@ -10,3 +10,4 @@ fn main() {");
    assert_eq!(plain[2], "  10│  10│  context");
    assert_eq!(plain[3], "  11│    │ -old");
    assert_eq!(plain[4], "    │  11│ +new");
    assert_eq!(plain[5], "    │  12│ +extra");
}

#[test]
fn git_diff_line_numbers_are_sidebar_only() {
    let mut tc = make_tc("git", Some("@@ -1 +1 @@\n-old\n+new\n"));
    tc.details = json!({ "action": "diff" });

    let lines = styled_tool_output_lines(&tc, &Highlighter::new(), &Theme::default(), false);
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();

    assert_eq!(plain, vec!["@@ -1 +1 @@", "-old", "+new"]);
}
