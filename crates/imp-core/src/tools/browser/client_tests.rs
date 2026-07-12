use super::*;

#[test]
fn takes_one_line_and_preserves_buffered_suffix() {
    let mut buffer = "first\r\nsecond\n".to_string();
    assert_eq!(take_line(&mut buffer).as_deref(), Some("first"));
    assert_eq!(buffer, "second\n");
}

#[test]
fn response_limit_error_reports_configured_bound() {
    assert_eq!(
        response_limit_error(4096),
        "Lightpanda response exceeded 4096 bytes"
    );
}

#[test]
fn parses_text_tool_result() {
    let result = parse_tool_result(json!({
        "content": [{"type": "text", "text": "page state"}],
        "isError": false
    }))
    .unwrap();
    assert_eq!(result.text, "page state");
    assert!(!result.is_error);
}

#[test]
fn rejects_tool_result_without_content() {
    let error = parse_tool_result(json!({})).unwrap_err();
    assert!(error.contains("content"));
}
