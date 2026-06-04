use super::*;

fn make_tool(id: &str) -> DisplayToolCall {
    DisplayToolCall {
        id: id.into(),
        name: "read".into(),
        args_summary: "src/main.rs".into(),
        output: Some("fn main() {}".into()),
        details: serde_json::json!({"path": "src/main.rs"}),
        is_error: false,
        expanded: false,
        streaming_lines: Vec::new(),
        streaming_output: String::new(),
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn large_pasted_code_is_summarized_for_display() {
    let code = (1..=25)
        .map(|i| format!("fn example_{i}() {{}}"))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(summarize_user_text_for_display(&code), "[Pasted 25 Lines]");
}

#[test]
fn ordinary_multiline_text_is_not_summarized() {
    let text = (1..=25)
        .map(|i| format!("This is regular prose line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(summarize_user_text_for_display(&text), text);
}

#[test]
fn short_code_block_is_not_summarized() {
    let code = (1..=2)
        .map(|i| format!("let value_{i} = {i};"))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(summarize_user_text_for_display(&code), code);
}

#[test]
fn three_line_code_block_is_summarized() {
    let code = (1..=3)
        .map(|i| format!("let value_{i} = {i};"))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(summarize_user_text_for_display(&code), "[Pasted 3 Lines]");
}

#[test]
fn wraps_long_user_message() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::User,
        content: "this is a long line that should wrap in the chat view".into(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        20,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    assert!(lines.len() > 2, "expected wrapped content plus separator");
}

#[test]
fn hide_tools_in_chat_removes_tool_lines() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: "done".into(),
        thinking: None,
        tool_calls: vec![make_tool("tc-1")],
        assistant_blocks: Vec::new(),
        is_streaming: false,
        timestamp: 0,
    }];

    let (_, visible_tools) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Hidden,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    assert!(visible_tools.is_empty());
}

#[test]
fn build_click_map_from_rendered_lines_finds_visible_tool_headers() {
    let lines = vec![
        Line::from("hello"),
        Line::from("  ▸ #tool-1 read src/main.rs"),
        Line::from("world"),
    ];
    let map = build_click_map_from_rendered_lines(&lines, Rect::new(0, 10, 80, 3), 0);
    assert_eq!(map, vec![(11, "tool-1".to_string())]);
}

#[test]
fn build_click_map_from_rendered_lines_respects_scroll_window() {
    let lines = vec![
        Line::from("before"),
        Line::from("▸ #tool-1 read src/main.rs"),
        Line::from("middle"),
        Line::from("▾ #tool-2 bash cargo test"),
    ];
    let map = build_click_map_from_rendered_lines(&lines, Rect::new(0, 5, 80, 2), 0);
    assert_eq!(map, vec![(6, "tool-2".to_string())]);
}

#[test]
fn assistant_blocks_preserve_thought_duration_tool_thought_order() {
    let display = DisplayMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        thinking: None,
        tool_calls: vec![make_tool("tc-1")],
        assistant_blocks: vec![
            DisplayAssistantBlock::ThoughtDuration { seconds: 5 },
            DisplayAssistantBlock::ToolCall { id: "tc-1".into() },
            DisplayAssistantBlock::ThoughtDuration { seconds: 20 },
            DisplayAssistantBlock::Text("Done".into()),
        ],
        is_streaming: false,
        timestamp: 0,
    };

    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let (lines, _) = build_chat_lines(
        &[display],
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    let first_thought_idx = rendered
        .iter()
        .position(|line| line.contains("thought for 5 seconds"))
        .unwrap();
    let tool_idx = rendered
        .iter()
        .position(|line| line.contains("Read") && line.contains("src/main.rs"))
        .unwrap();
    let second_thought_idx = rendered
        .iter()
        .position(|line| line.contains("thought for 20 seconds"))
        .unwrap();
    let text_idx = rendered
        .iter()
        .position(|line| line.contains("Done"))
        .unwrap();

    assert!(first_thought_idx < tool_idx);
    assert!(tool_idx < second_thought_idx);
    assert!(second_thought_idx < text_idx);
}

#[test]
fn assistant_blocks_preserve_text_tool_text_order() {
    let assistant = imp_llm::Message::Assistant(imp_llm::AssistantMessage {
        content: vec![
            imp_llm::ContentBlock::Text {
                text: "Before tool".into(),
            },
            imp_llm::ContentBlock::ToolCall {
                id: "tc-1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            },
            imp_llm::ContentBlock::Text {
                text: "After tool".into(),
            },
        ],
        usage: None,
        stop_reason: imp_llm::StopReason::ToolUse,
        timestamp: 0,
    });

    let display = DisplayMessage::from_message(&assistant);
    assert_eq!(
        display.assistant_blocks,
        vec![
            DisplayAssistantBlock::Text("Before tool".into()),
            DisplayAssistantBlock::ToolCall { id: "tc-1".into() },
            DisplayAssistantBlock::Text("After tool".into()),
        ]
    );
}

#[test]
fn interleaved_mode_renders_tool_between_text_blocks() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: "Before toolAfter tool".into(),
        thinking: None,
        tool_calls: vec![make_tool("tc-1")],
        assistant_blocks: vec![
            DisplayAssistantBlock::Text("Before tool".into()),
            DisplayAssistantBlock::ToolCall { id: "tc-1".into() },
            DisplayAssistantBlock::Text("After tool".into()),
        ],
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    let before_idx = rendered
        .iter()
        .position(|line| line.contains("Before tool"))
        .unwrap();
    let tool_idx = rendered
        .iter()
        .position(|line| line.contains("Read") && line.contains("src/main.rs"))
        .unwrap();
    let after_idx = rendered
        .iter()
        .position(|line| line.contains("After tool"))
        .unwrap();

    assert!(before_idx < tool_idx && tool_idx < after_idx);
}

#[test]
fn summary_mode_hides_tool_output_but_keeps_header() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let mut tool = make_tool("tc-1");
    tool.expanded = true;
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        thinking: None,
        tool_calls: vec![tool],
        assistant_blocks: vec![DisplayAssistantBlock::ToolCall { id: "tc-1".into() }],
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, visible_tools) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Summary,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(visible_tools.len(), 1);
    assert!(rendered
        .iter()
        .any(|line| line.contains("Read") && line.contains("src/main.rs")));
    assert!(!rendered.iter().any(|line| line.contains("fn main() {}")));
}

#[test]
fn focused_tool_call_shows_arrow_in_summary_mode() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        thinking: None,
        tool_calls: vec![make_tool("tc-1")],
        assistant_blocks: vec![DisplayAssistantBlock::ToolCall { id: "tc-1".into() }],
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, visible_tools) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        Some(0),
        true,
        ChatToolDisplay::Summary,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert_eq!(visible_tools.len(), 1);
    assert!(rendered
        .iter()
        .any(|line| line.contains("▸") && line.contains("Read") && line.contains("src/main.rs")));
}

#[test]
fn streaming_placeholder_renders_waiting_in_chat() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: true,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::WaitingForResponse,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(rendered.iter().any(|line| line.contains("waiting")));
}

#[test]
fn streaming_placeholder_renders_responding_in_chat() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: true,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Streaming,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(rendered.iter().any(|line| line.contains("responding")));
}

#[test]
fn warning_messages_render_with_prefix() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::Warning,
        content: "line 1\nline 2".into(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(rendered.iter().any(|line| line.contains("Warning: line 1")));
    assert!(rendered.iter().any(|line| line.contains("Warning: line 2")));
}

#[test]
fn system_messages_render_all_lines() {
    let theme = Theme::default();
    let highlighter = Highlighter::new();
    let messages = vec![DisplayMessage {
        role: MessageRole::System,
        content: "line 1\nline 2\nline 3\nline 4".into(),
        thinking: None,
        tool_calls: Vec::new(),
        assistant_blocks: Vec::new(),
        is_streaming: false,
        timestamp: 0,
    }];

    let (lines, _) = build_chat_lines(
        &messages,
        &theme,
        &highlighter,
        80,
        0,
        None,
        true,
        ChatToolDisplay::Interleaved,
        5,
        false,
        AnimationLevel::Minimal,
        AnimationState::Idle,
    );

    let rendered: Vec<String> = lines.iter().map(line_text).collect();
    assert!(rendered.iter().any(|line| line.contains("line 1")));
    assert!(rendered.iter().any(|line| line.contains("line 2")));
    assert!(rendered.iter().any(|line| line.contains("line 3")));
    assert!(rendered.iter().any(|line| line.contains("line 4")));
}
