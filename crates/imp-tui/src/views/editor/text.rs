use unicode_width::UnicodeWidthChar;

pub(super) fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos;
    while p > 0 {
        p -= 1;
        if s.is_char_boundary(p) {
            return p;
        }
    }
    0
}

pub(super) fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p < s.len() {
        p += 1;
        if s.is_char_boundary(p) {
            return p;
        }
    }
    s.len()
}

pub fn clamp_cursor_to_boundary(text: &str, cursor: usize) -> usize {
    let mut clamped = cursor.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

pub(super) fn line_col_to_byte(lines: &[&str], line: usize, col: usize) -> usize {
    let mut byte = 0;
    for (i, l) in lines.iter().enumerate() {
        if i == line {
            return byte + col.min(l.len());
        }
        byte += l.len() + 1; // +1 for \n
    }
    byte
}

pub fn wrapped_lines_for_width(text: &str, inner_width: u16) -> Vec<String> {
    let width = inner_width.max(1) as usize;
    let mut out = Vec::new();

    for logical in text.split('\n') {
        if logical.is_empty() {
            out.push(String::new());
            continue;
        }

        wrap_logical_line(logical, width, &mut out);
    }

    if out.is_empty() {
        out.push(String::new());
    }

    out
}

fn wrap_logical_line(logical: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut last_whitespace_byte = None;

    for ch in logical.chars() {
        let ch_width = char_display_width(ch);

        if !current.is_empty() && current_width + ch_width > width {
            if let Some(split_byte) = last_whitespace_byte {
                let next = current[split_byte..].trim_start().to_string();
                let line = current[..split_byte].trim_end().to_string();

                if !line.is_empty() {
                    out.push(line);
                }

                current = next;
                current_width = display_width(&current);
                last_whitespace_byte = last_whitespace_byte_in(&current);
            } else {
                out.push(current);
                current = String::new();
                current_width = 0;
                last_whitespace_byte = None;
            }
        }

        if current.is_empty() && ch_width > width {
            out.push(ch.to_string());
            continue;
        }

        current.push(ch);
        current_width += ch_width;

        if ch.is_whitespace() {
            last_whitespace_byte = Some(current.len());
        }

        if current_width == width {
            if let Some(split_byte) = last_whitespace_byte {
                let next = current[split_byte..].trim_start().to_string();
                let line = current[..split_byte].trim_end().to_string();

                if !line.is_empty() {
                    out.push(line);
                }

                current = next;
                current_width = display_width(&current);
                last_whitespace_byte = last_whitespace_byte_in(&current);
            } else {
                out.push(current);
                current = String::new();
                current_width = 0;
                last_whitespace_byte = None;
            }
        }
    }

    if !current.is_empty() {
        out.push(current);
    }
}

pub(super) fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn last_whitespace_byte_in(text: &str) -> Option<usize> {
    text.char_indices()
        .filter_map(|(idx, ch)| ch.is_whitespace().then_some(idx + ch.len_utf8()))
        .next_back()
}

pub fn cursor_visual_position_for_text(
    text: &str,
    cursor: usize,
    inner_width: u16,
) -> (usize, usize) {
    let cursor = clamp_cursor_to_boundary(text, cursor);
    let before_cursor = &text[..cursor];
    let lines = wrapped_lines_for_width(before_cursor, inner_width);
    let row = lines.len().saturating_sub(1);
    let col = lines.last().map(|line| display_width(line)).unwrap_or(0);

    (row, col)
}

pub(super) fn char_display_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        _ => ch.width().unwrap_or(1).max(1),
    }
}
