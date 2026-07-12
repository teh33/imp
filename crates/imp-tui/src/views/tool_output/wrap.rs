use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub fn wrap_styled_lines(lines: &[Line<'static>], width: usize) -> Vec<Line<'static>> {
    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap_line(line, width));
    }
    wrapped
}

fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line.clone()];
    }
    let chars = flatten_line_chars(line);
    if chars.len() <= width {
        return vec![line.clone()];
    }
    wrap_styled_chars(&chars, width)
        .into_iter()
        .map(|chunk| Line::from(chars_to_spans(&chunk)))
        .collect()
}

fn flatten_line_chars(line: &Line<'static>) -> Vec<(char, Style)> {
    let mut chars = Vec::new();
    for span in &line.spans {
        let style = span.style;
        for ch in span.content.chars() {
            chars.push((ch, style));
        }
    }
    chars
}

fn wrap_styled_chars(chars: &[(char, Style)], width: usize) -> Vec<Vec<(char, Style)>> {
    if chars.is_empty() {
        return vec![Vec::new()];
    }
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_len = 0;
    for (ch, style) in chars {
        if *ch == '\n' {
            lines.push(current);
            current = Vec::new();
            current_len = 0;
            continue;
        }
        if current_len >= width {
            lines.push(current);
            current = Vec::new();
            current_len = 0;
        }
        current.push((*ch, *style));
        current_len += 1;
    }
    lines.push(current);
    lines
}

fn chars_to_spans(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, style) in chars {
        if let Some(last) = spans.last_mut() {
            if last.style == *style {
                last.content.to_mut().push(*ch);
                continue;
            }
        }
        spans.push(Span::styled(ch.to_string(), *style));
    }
    spans
}
