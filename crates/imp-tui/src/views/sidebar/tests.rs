use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

// ── Sidebar state ───────────────────────────────────────────

#[test]
fn sidebar_default_state() {
    let sidebar = Sidebar::default();
    assert!(!sidebar.open);
    assert_eq!(sidebar.list_scroll, 0);
    assert_eq!(sidebar.detail_scroll, 0);
    assert!(!sidebar.first_tool_seen);
}

#[test]
fn sidebar_summary_expands_nested_details() {
    let mut tc = make_tc("work", "", None, false);
    tc.details = serde_json::json!({
        "action": "list",
        "items": [{
            "id": "T-improve-sidebar-detail",
            "title": "Improve sidebar detail",
            "status": "ready"
        }],
        "policy": { "decision": "allowed", "tool_name": "work" }
    });

    let rows = tool_input_summary_rows(&tc);
    assert!(rows.iter().any(|row| row.contains("action: list")));
    assert!(!rows.iter().any(|row| row.contains("policy")));
    assert!(!rows.iter().any(|row| row.contains("{3 fields}")));
}

#[test]
fn card_tool_sidebar_detail_skips_raw_header_and_input_dump() {
    for (name, expected_header) in [
        ("work", "▣Work · create"),
        ("read", "◧Read"),
        ("edit", "◇Edit"),
    ] {
        let mut tc = make_tc(name, "", Some("output"), false);
        tc.details = match name {
            "work" => serde_json::json!({
                "action": "create",
                "kind": "task",
                "id": "T-test-work-unit",
                "item": {
                    "id": "T-test-work-unit",
                    "title": "Test work unit",
                    "status": "todo"
                }
            }),
            "read" => serde_json::json!({"path": "/tmp/example.txt"}),
            "edit" => serde_json::json!({"path": "/tmp/example.txt"}),
            _ => serde_json::Value::Null,
        };

        let lines = styled_detail_lines(
            Some(&tc),
            &UiConfig::default(),
            &Highlighter::new(),
            &Theme::default(),
            80,
        );
        let plain = lines
            .iter()
            .map(|line| line_to_plain_text(line))
            .collect::<Vec<_>>();

        assert_eq!(plain.first().map(String::as_str), Some(expected_header));
        assert!(!plain.iter().any(|line| line == "input"));
        assert!(!plain.iter().any(|line| line.starts_with("path:")));
        assert!(!plain.iter().any(|line| line.starts_with("action:")));
    }
}

#[test]
fn sidebar_scroll_list() {
    let mut sidebar = Sidebar::default();
    sidebar.scroll_list_down(5);
    assert_eq!(sidebar.list_scroll, 5);
    sidebar.scroll_list_up(3);
    assert_eq!(sidebar.list_scroll, 2);
    sidebar.scroll_list_up(10);
    assert_eq!(sidebar.list_scroll, 0);
}

#[test]
fn sidebar_scroll_detail() {
    let mut sidebar = Sidebar::default();
    sidebar.scroll_detail_down(5);
    assert_eq!(sidebar.detail_scroll, 5);
    sidebar.scroll_detail_up(3);
    assert_eq!(sidebar.detail_scroll, 2);
    sidebar.scroll_detail_up(10);
    assert_eq!(sidebar.detail_scroll, 0);
}

#[test]
fn sidebar_ensure_selected_visible_scrolls_down() {
    let mut sidebar = Sidebar {
        list_height: 5,
        ..Sidebar::default()
    };
    sidebar.ensure_selected_visible(7);
    assert!(sidebar.list_scroll + 5 > 7);
}

#[test]
fn sidebar_ensure_selected_visible_scrolls_up() {
    let mut sidebar = Sidebar {
        list_height: 5,
        list_scroll: 10,
        ..Sidebar::default()
    };
    sidebar.ensure_selected_visible(3);
    assert_eq!(sidebar.list_scroll, 3);
}

// ── Layout ──────────────────────────────────────────────────

#[test]
fn compute_split_too_small() {
    let area = Rect::new(0, 0, 40, 4);
    let (list, sep, detail) = compute_split(area, 5);
    assert_eq!(list.height, 4);
    assert!(sep.is_none());
    assert_eq!(detail.height, 0);
}

#[test]
fn compute_split_few_tools() {
    let area = Rect::new(0, 0, 40, 20);
    let (list, sep, detail) = compute_split(area, 3);
    assert!(sep.is_some());
    assert!(list.height >= 2);
    assert!(detail.height >= 3);
    assert_eq!(list.height as usize + 1 + detail.height as usize, 20);
}

#[test]
fn sidebar_sub_areas_stream_covers_full() {
    let sidebar = Rect::new(50, 0, 30, 20);
    let (top, bottom) = sidebar_sub_areas(sidebar, 5, SidebarStyle::Stream);
    assert_eq!(top.height, 20);
    assert_eq!(bottom.height, 0);
}

#[test]
fn sidebar_sub_areas_split_has_two_regions() {
    let sidebar = Rect::new(50, 0, 30, 20);
    let (top, bottom) = sidebar_sub_areas(sidebar, 5, SidebarStyle::Split);
    assert!(top.height > 0);
    assert!(bottom.height > 0);
}

#[test]
fn format_workflow_output_renders_summary_and_units() {
    let tc = DisplayToolCall {
        id: "1".into(),
        name: "workflow".into(),
        args_summary: "run".into(),
        output: None,
        details: serde_json::json!({
            "action": "run",
            "jobs": 4,
            "background": true,
            "view": {
                "summary": {
                    "total_units": 3,
                    "total_closed": 2,
                    "total_failed": 1,
                    "total_awaiting_verify": 0,
                    "total_skipped": 0
                },
                "units": [
                    {"id": "1.1", "title": "First", "status": "done", "round": 1, "duration_secs": 8},
                    {"id": "1.2", "title": "Second", "status": "failed", "round": 1}
                ]
            }
        }),
        is_error: false,
        expanded: false,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    };

    let lines = format_workflow_output(&tc);
    assert_eq!(lines[0], "request");
    assert!(lines.iter().any(|l| l == "  action run"));
    assert!(lines.iter().any(|l| l == "  jobs 4"));
    assert!(lines.iter().any(|l| l == "  background true"));
    assert!(lines.iter().any(|l| l == "summary"));
    assert!(lines
        .iter()
        .any(|l| l.contains("3 units · 2 done · 1 failed")));
    assert!(!lines.iter().any(|l| l.contains("verify")));
    assert!(lines.iter().any(|l| l == "units"));
    assert!(lines.iter().any(|l| l.contains("✓ 1.1 · First")));
    assert!(lines.iter().any(|l| l.contains("done · wave 1 · 8s")));
    assert!(lines.iter().any(|l| l.contains("✗ 1.2 · Second")));
    assert!(lines.iter().any(|l| l.contains("failed · wave 1")));
}

#[test]
fn format_workflow_output_renders_scope_target_and_runtime() {
    let tc = DisplayToolCall {
        id: "run-1".into(),
        name: "workflow".into(),
        args_summary: "run".into(),
        output: None,
        details: serde_json::json!({
            "action": "run",
            "scope": "targets 1, 2",
            "target": {"kind": "explicit", "ids": ["1", "2"]},
            "runtime": {"direct_agent": "imp", "model": "sonnet"},
            "background": true,
            "view": {
                "summary": {
                    "total_units": 2,
                    "total_closed": 2,
                    "total_failed": 0,
                    "total_awaiting_verify": 0,
                    "total_skipped": 0
                },
                "units": []
            }
        }),
        is_error: false,
        expanded: false,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    };

    let lines = format_workflow_output(&tc);
    assert!(lines.iter().any(|l| l == "  scope targets 1, 2"));
    assert!(lines.iter().any(|l| l == "  target explicit: 1, 2"));
    assert!(lines.iter().any(|l| l == "  runtime imp · sonnet"));
}

#[test]
fn format_workflow_output_renders_delta_actions() {
    let tc = DisplayToolCall {
        id: "delta-1".into(),
        name: "workflow".into(),
        args_summary: "decision_add".into(),
        output: Some("workflow delta: decision added on 1 · Test unit".into()),
        details: serde_json::json!({
            "action": "decision_add",
            "id": "1",
            "description": "Choose retry limit",
            "unit": {
                "id": "1",
                "title": "Test unit",
                "status": "open",
                "decisions": ["Choose retry limit"]
            }
        }),
        is_error: false,
        expanded: false,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    };

    let lines = format_workflow_output(&tc);
    assert!(lines.iter().any(|l| l == "  action decision_add"));
    assert!(lines.iter().any(|l| l == "  id 1"));
    assert!(lines
        .iter()
        .any(|l| l == "  description Choose retry limit"));
    assert!(lines.iter().any(|l| l == "  unit 1 · Test unit · open"));
    assert!(lines
        .iter()
        .any(|l| l.contains("workflow delta: decision added on 1 · Test unit")));
}

#[test]
fn wrap_short_line_unchanged() {
    let mut out = Vec::new();
    wrap_into("hello", 10, &mut out);
    assert_eq!(out, vec!["hello"]);
}

#[test]
fn wrap_at_space() {
    let mut out = Vec::new();
    wrap_into("hello world foo", 11, &mut out);
    assert_eq!(out, vec!["hello world", "foo"]);
}

#[test]
fn wrap_long_word_force_break() {
    let mut out = Vec::new();
    wrap_into("abcdefghij", 4, &mut out);
    assert_eq!(out, vec!["abcd", "efgh", "ij"]);
}

#[test]
fn wrap_empty() {
    let mut out = Vec::new();
    wrap_into("", 10, &mut out);
    assert_eq!(out, vec![""]);
}

#[test]
fn inspector_sidebar_uses_full_area_for_detail() {
    let area = Rect::new(10, 2, 40, 12);
    let (list, detail) = sidebar_sub_areas(area, 3, SidebarStyle::Inspector);

    assert_eq!(list, detail);
    assert_eq!(detail.x, area.x);
    assert_eq!(detail.width, area.width);
    assert_eq!(detail.y, area.y);
    assert_eq!(detail.height, area.height);
}

// ── Tool output lines ───────────────────────────────────────

fn make_tc(name: &str, args: &str, output: Option<&str>, is_error: bool) -> DisplayToolCall {
    DisplayToolCall {
        id: format!("tc-{name}"),
        name: name.into(),
        args_summary: args.into(),
        output: output.map(String::from),
        details: serde_json::Value::Null,
        is_error,
        expanded: false,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    }
}

#[test]
fn inspector_detail_includes_selected_tool_header_and_full_output() {
    let tc = make_tc("bash", "$ printf", Some("line1\nline2"), false);
    let config = UiConfig {
        sidebar_style: SidebarStyle::Inspector,
        tool_output: ToolOutputDisplay::Compact,
        tool_output_lines: 1,
        word_wrap: false,
        ..Default::default()
    };

    let render = build_detail_render_data(
        Some(&tc),
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        80,
    );

    assert!(render
        .plain_lines
        .iter()
        .any(|line| line.contains("Terminal")));
    assert!(render.plain_lines.iter().any(|line| line == "line1"));
    assert!(render.plain_lines.iter().any(|line| line == "line2"));
    assert!(!render.plain_lines.iter().any(|line| line == "…"));
}

#[test]
fn inspector_detail_includes_tool_input_arguments() {
    let mut tc = make_tc("shell", "run", Some("done"), false);
    tc.details = serde_json::json!({
        "command": "cargo test -p imp-tui inspector -- --nocapture",
        "timeout": 120000,
    });
    let config = UiConfig {
        sidebar_style: SidebarStyle::Inspector,
        tool_output: ToolOutputDisplay::Compact,
        tool_output_lines: 1,
        word_wrap: false,
        ..Default::default()
    };

    let render = build_detail_render_data(
        Some(&tc),
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        120,
    );

    assert!(render
        .plain_lines
        .iter()
        .any(|line| line.contains("command: cargo test -p imp-tui inspector")));
    assert!(render.plain_lines.iter().any(|line| line == "done"));
}

#[test]
fn inspector_detail_summarizes_large_tool_input_arguments() {
    let mut tc = make_tc("edit", "run", Some("done"), false);
    tc.details = serde_json::json!({
        "edits": (0..120).map(|idx| serde_json::json!({
            "oldText": format!("old-{idx}"),
            "newText": "x".repeat(10_000),
        })).collect::<Vec<_>>(),
    });

    let render = build_detail_render_data(
        Some(&tc),
        &UiConfig {
            sidebar_style: SidebarStyle::Inspector,
            word_wrap: true,
            ..Default::default()
        },
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        40,
    );

    assert!(render
        .plain_lines
        .iter()
        .any(|line| line.contains("edits: 120")));
    assert!(!render
        .plain_lines
        .iter()
        .any(|line| line.contains("old-119")));
    assert!(render.plain_lines.iter().all(|line| line.len() < 1_000));
}

#[test]
fn styled_output_lines_read_include_numbered_source() {
    let mut tc = make_tc("read", "f.rs", Some("fn main() {}"), false);
    tc.details = serde_json::json!({"path": "src/main.rs", "lines": 1});
    let config = UiConfig {
        tool_output: ToolOutputDisplay::Full,
        word_wrap: false,
        ..Default::default()
    };
    let lines = styled_output_lines(
        &tc,
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        80,
    );
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();
    assert!(plain.iter().any(|line| line.starts_with("  1│")));
    assert!(plain.iter().any(|line| line.contains("fn main()")));
}

#[test]
fn styled_output_lines_use_live_streaming_output_in_sidebar() {
    let mut tc = make_tc("bash", "$ echo hi", None, false);
    tc.streaming_output = "line 1\nline 2".into();
    let config = UiConfig {
        tool_output: ToolOutputDisplay::Full,
        word_wrap: false,
        ..Default::default()
    };

    let lines = styled_output_lines(
        &tc,
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        80,
    );
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();
    assert_eq!(plain, vec!["line 1".to_string(), "line 2".to_string()]);
}

#[test]
fn styled_output_lines_write_show_file_content() {
    let mut tc = make_tc("write", "f.rs", Some("summary only"), false);
    tc.details = serde_json::json!({
        "path": "src/lib.rs",
        "summary": "src/lib.rs: 12 bytes created",
        "display_content": "pub fn hi() {}",
        "display_note": ""
    });
    let config = UiConfig {
        tool_output: ToolOutputDisplay::Full,
        word_wrap: false,
        ..Default::default()
    };
    let lines = styled_output_lines(
        &tc,
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        80,
    );
    let plain: Vec<String> = lines
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content).collect())
        .collect();
    assert!(plain.iter().any(|line| line.contains("pub fn hi")));
}

#[test]
fn styled_output_lines_wrap_long_plain_lines() {
    let tc = make_tc(
        "bash",
        "$ echo",
        Some("this is a very long line that should wrap inside the sidebar viewer"),
        false,
    );
    let config = UiConfig {
        tool_output: ToolOutputDisplay::Full,
        word_wrap: true,
        ..Default::default()
    };

    let lines = styled_output_lines(
        &tc,
        &config,
        &crate::highlight::Highlighter::new(),
        &Theme::default(),
        20,
    );

    assert!(lines.len() > 1);
}

// ── Widget rendering ────────────────────────────────────────

#[test]
fn build_detail_text_surface_uses_full_area_without_header_offset() {
    let tc = make_tc("bash", "$ ls", Some("line1\nline2\nline3"), false);
    let config = UiConfig {
        sidebar_style: SidebarStyle::Split,
        word_wrap: false,
        ..Default::default()
    };
    let area = Rect::new(10, 5, 30, 6);

    let theme = Theme::default();
    let highlighter = crate::highlight::Highlighter::new();
    let surface = build_detail_text_surface(Some(&tc), area, 0, &config, &highlighter, &theme);

    assert_eq!(surface.rect, area);
}

#[test]
fn sidebar_view_empty_no_panic() {
    let theme = Theme::default();
    let config = UiConfig::default();
    let highlighter = crate::highlight::Highlighter::new();
    let view = SidebarView::new(vec![], None, &theme, &highlighter, 0, 0, 0, &config);
    let area = Rect::new(0, 0, 40, 10);
    let mut buf = Buffer::empty(area);
    view.render(area, &mut buf);
}

#[test]
fn sidebar_view_stream_mode_no_panic() {
    let theme = Theme::default();
    let config = UiConfig {
        sidebar_style: SidebarStyle::Stream,
        ..Default::default()
    };
    let tc1 = make_tc("read", "file.rs", Some("fn main() {}"), false);
    let tc2 = make_tc("bash", "$ ls", Some("file1\nfile2"), false);
    let highlighter = crate::highlight::Highlighter::new();
    let view = SidebarView::new(
        vec![&tc1, &tc2],
        Some(0),
        &theme,
        &highlighter,
        0,
        0,
        0,
        &config,
    );
    let area = Rect::new(0, 0, 50, 20);
    let mut buf = Buffer::empty(area);
    view.render(area, &mut buf);
}

#[test]
fn sidebar_view_split_mode_no_panic() {
    let theme = Theme::default();
    let config = UiConfig {
        sidebar_style: SidebarStyle::Split,
        ..Default::default()
    };
    let tc1 = make_tc("read", "file.rs", Some("fn main() {}"), false);
    let tc2 = make_tc("bash", "$ ls", Some("file1\nfile2"), false);
    let highlighter = crate::highlight::Highlighter::new();
    let view = SidebarView::new(
        vec![&tc1, &tc2],
        Some(1),
        &theme,
        &highlighter,
        0,
        0,
        0,
        &config,
    );
    let area = Rect::new(0, 0, 50, 20);
    let mut buf = Buffer::empty(area);
    view.render(area, &mut buf);
}

#[test]
fn sidebar_view_tiny_no_panic() {
    let theme = Theme::default();
    let config = UiConfig::default();
    let tc = make_tc("read", "f.rs", Some("hello"), false);
    let highlighter = crate::highlight::Highlighter::new();
    let view = SidebarView::new(vec![&tc], Some(0), &theme, &highlighter, 0, 0, 0, &config);
    let area = Rect::new(0, 0, 2, 1);
    let mut buf = Buffer::empty(area);
    view.render(area, &mut buf);
}
