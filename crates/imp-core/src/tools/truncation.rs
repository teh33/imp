use std::path::PathBuf;

pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub output_lines: usize,
    pub total_lines: usize,
    pub output_bytes: usize,
    pub total_bytes: usize,
    pub temp_file: Option<PathBuf>,
}

/// Truncate a single line to max_bytes, appending "…" if truncated.
pub fn truncate_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes.min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

/// Write full content to a temp file, returning the path.
fn write_temp_file(content: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join("imp-tools");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("truncated-{}.txt", uuid::Uuid::new_v4());
    let path = dir.join(name);
    std::fs::write(&path, content).ok()?;
    Some(path)
}

/// Truncate keeping the head (first N lines/bytes).
/// When truncated, writes full output to a temp file.
pub fn truncate_head(input: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines: Vec<&str> = input.lines().collect();
    let total_lines = lines.len();
    let total_bytes = input.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: input.to_string(),
            truncated: false,
            output_lines: total_lines,
            total_lines,
            output_bytes: total_bytes,
            total_bytes,
            temp_file: None,
        };
    }

    let mut result = String::new();
    let mut byte_count = 0;
    let mut line_count = 0;

    for line in &lines {
        let line_with_newline = format!("{line}\n");
        if line_count >= max_lines || byte_count + line_with_newline.len() > max_bytes {
            break;
        }
        result.push_str(&line_with_newline);
        byte_count += line_with_newline.len();
        line_count += 1;
    }

    let temp_file = write_temp_file(input);

    TruncationResult {
        content: result,
        truncated: true,
        output_lines: line_count,
        total_lines,
        output_bytes: byte_count,
        total_bytes,
        temp_file,
    }
}

/// Truncate keeping the tail (last N lines/bytes).
/// When truncated, writes full output to a temp file.
pub fn truncate_tail(input: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines: Vec<&str> = input.lines().collect();
    let total_lines = lines.len();
    let total_bytes = input.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: input.to_string(),
            truncated: false,
            output_lines: total_lines,
            total_lines,
            output_bytes: total_bytes,
            total_bytes,
            temp_file: None,
        };
    }

    // Walk backwards from the end, collecting lines that fit.
    let start = total_lines.saturating_sub(max_lines);
    let mut actual_start = start;
    let mut remaining_bytes = max_bytes;

    for (i, line) in lines[start..].iter().enumerate() {
        let line_with_newline = format!("{line}\n");
        if line_with_newline.len() > remaining_bytes {
            actual_start = start + i + 1;
            remaining_bytes = max_bytes;
            // Recalculate from new start
            for line2 in &lines[actual_start..] {
                let l = format!("{line2}\n");
                if l.len() > remaining_bytes {
                    break;
                }
                remaining_bytes -= l.len();
            }
            break;
        }
        remaining_bytes -= line_with_newline.len();
    }

    let mut result = String::new();
    for line in &lines[actual_start..] {
        result.push_str(&format!("{line}\n"));
    }

    let output_lines = total_lines - actual_start;
    let output_bytes = result.len();
    let temp_file = write_temp_file(input);

    TruncationResult {
        content: result,
        truncated: true,
        output_lines,
        total_lines,
        output_bytes,
        total_bytes,
        temp_file,
    }
}
