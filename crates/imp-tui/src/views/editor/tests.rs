use super::*;
use ratatui::layout::Rect;

#[test]
fn format_context_usage_prefers_percent_over_current_tokens() {
    assert_eq!(format_context_usage(82_400, 1_000_000), "8%/1.0M");
    assert_eq!(format_context_usage(500_000, 1_000_000), "50%/1.0M");
}

#[test]
fn format_compact_tokens_handles_millions() {
    assert_eq!(format_compact_tokens(1_000_000), "1.0M");
    assert_eq!(format_compact_tokens(1_250_000), "1.2M");
}

#[test]
fn format_compact_tokens_handles_thousands() {
    assert_eq!(format_compact_tokens(9_500), "9.50k");
    assert_eq!(format_compact_tokens(12_300), "12.3k");
    assert_eq!(format_compact_tokens(234_000), "234k");
}

#[test]
fn wrapped_lines_prefer_word_boundaries() {
    assert_eq!(
        wrapped_lines_for_width("hello world", 8),
        vec!["hello".to_string(), "world".to_string()]
    );
}

#[test]
fn wrapped_lines_split_words_that_exceed_width() {
    assert_eq!(
        wrapped_lines_for_width("superlongword", 5),
        vec!["super".to_string(), "longw".to_string(), "ord".to_string()]
    );
}

#[test]
fn cursor_position_tracks_word_boundary_wraps() {
    assert_eq!(
        cursor_visual_position_for_text("hello world", 11, 8),
        (1, 5)
    );
}

#[test]
fn cursor_position_tracks_partially_wrapped_word() {
    assert_eq!(cursor_visual_position_for_text("hello world", 9, 8), (1, 3));
}

#[test]
fn visual_line_count_includes_soft_wraps() {
    let mut editor = EditorState::new();
    editor.set_content("abcdefghij");

    assert_eq!(editor.visual_line_count(4), 3);
}

#[test]
fn typed_long_code_is_not_summarized() {
    let mut editor = EditorState::new();
    editor.set_content(
        &(1..=25)
            .map(|i| format!("fn example_{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    assert_eq!(editor.visual_line_count_with_summary(80, true), 25);
}

#[test]
fn pasted_code_summary_preserves_surrounding_prompt_text() {
    let mut editor = EditorState::new();
    editor.set_content("please inspect:\n");
    editor.insert_paste(
        &(1..=5)
            .map(|i| format!("fn example_{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    editor.insert_newline();
    editor.insert_char('t');
    editor.insert_char('h');
    editor.insert_char('x');

    assert_eq!(
        editor_display_lines(&editor.content, &editor.paste_ranges, 80, true),
        vec![
            "please inspect:".to_string(),
            "[fn example_1() {} + 4 lines]".to_string(),
            "thx".to_string(),
        ]
    );
    assert!(editor.content().contains("fn example_5() {}"));
}

#[test]
fn cursor_screen_position_tracks_soft_wraps() {
    let mut editor = EditorState::new();
    editor.set_content("abcdefghij");

    let area = Rect::new(0, 0, 6, 5); // inner width = 4
    let (x, y) = editor.cursor_screen_position(area);

    assert_eq!((x, y), (3, 3));
}

#[test]
fn editor_operations_clamp_cursor_past_end() {
    let mut editor = EditorState::new();
    editor.set_content("abc");
    editor.cursor = 99;

    editor.delete_back();

    assert_eq!(editor.content(), "ab");
    assert_eq!(editor.cursor, 2);
}

#[test]
fn editor_operations_clamp_invalid_utf8_boundary() {
    let mut editor = EditorState::new();
    editor.set_content("éx");
    editor.cursor = 1; // inside 'é'

    editor.insert_char('!');

    assert_eq!(editor.content(), "!éx");
    assert!(editor.content().is_char_boundary(editor.cursor));
}

#[test]
fn cursor_screen_position_handles_tiny_area_without_underflow() {
    let mut editor = EditorState::new();
    editor.set_content("abc");
    editor.cursor = usize::MAX;

    let (x, y) = editor.cursor_screen_position(Rect::new(5, 7, 0, 0));
    assert_eq!((x, y), (5, 7));

    let (x, y) = editor.cursor_screen_position(Rect::new(5, 7, 1, 1));
    assert_eq!((x, y), (5, 7));
}

#[test]
fn abbreviate_home_prefers_tilde() {
    assert_eq!(abbreviate_home("/Users/asher/tower/imp"), "~/tower/imp");
    assert_eq!(abbreviate_home("/tmp/project"), "/tmp/project");
}

#[test]
fn identity_label_prefers_tilde_path() {
    let rendered = build_identity_label("/Users/asher/tower/imp", "chat", 80);
    let text: String = rendered
        .into_iter()
        .map(|span| span.content.into_owned())
        .collect();
    assert!(text.contains("~/tower/imp"));
    assert!(text.contains("chat"));
}

#[test]
fn bottom_left_label_uses_live_run_state_without_activity() {
    let rendered = build_bottom_left_label(
        WorkflowMode::Normal,
        Some("364 Test scope"),
        Some("run run-1 running"),
        None,
    );
    let text: String = rendered
        .into_iter()
        .map(|span| span.content.into_owned())
        .collect();
    assert!(!text.contains("BUILD"));
    assert!(text.contains("364 Test scope"));
    assert!(text.contains("run run-1 running"));
    assert!(!text.contains("working"));
}

#[test]
fn top_right_label_renders_elapsed() {
    let theme = Theme::default();
    let rendered = build_top_right_label(Some(Duration::from_secs(75)), &theme);
    let text: String = rendered
        .into_iter()
        .map(|span| span.content.into_owned())
        .collect();
    assert!(text.contains("1m15s"));
}

#[test]
fn bottom_left_label_hides_thinking_state() {
    let rendered = build_bottom_left_label(WorkflowMode::Normal, None, None, None);
    let text: String = rendered
        .into_iter()
        .map(|span| span.content.into_owned())
        .collect();
    assert_eq!(text, "");
}

#[test]
fn superbar_border_style_stays_static_when_active() {
    let theme = Theme::default();
    let idle = superbar_border_style(&theme, ThinkingLevel::Medium);
    let active = superbar_border_style(&theme, ThinkingLevel::Medium);
    assert_eq!(idle, active);
    assert_eq!(
        idle.fg,
        Some(theme.thinking_border_color(ThinkingLevel::Medium))
    );
    assert!(!idle.add_modifier.contains(ratatui::style::Modifier::BOLD));
}
