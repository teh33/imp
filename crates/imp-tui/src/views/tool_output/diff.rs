use ratatui::text::{Line, Span};

use crate::theme::Theme;

use super::git_line_style;

pub(super) fn git_diff_lines_with_line_numbers(
    output: &str,
    theme: &Theme,
    is_error: bool,
) -> Vec<Line<'static>> {
    let mut old_line: Option<usize> = None;
    let mut new_line: Option<usize> = None;
    let mut rendered = Vec::new();

    for raw_line in output.lines() {
        if let Some((old_start, new_start)) = parse_unified_hunk_header(raw_line) {
            old_line = Some(old_start);
            new_line = Some(new_start);
            rendered.push(Line::from(Span::styled(
                raw_line.to_string(),
                git_line_style(raw_line, theme, is_error),
            )));
            continue;
        }

        if is_diff_metadata_line(raw_line) || old_line.is_none() || new_line.is_none() {
            rendered.push(Line::from(Span::styled(
                raw_line.to_string(),
                git_line_style(raw_line, theme, is_error),
            )));
            continue;
        }

        let (old_label, new_label, advance_old, advance_new) = if raw_line.starts_with('+') {
            (String::new(), format_line_number(new_line), false, true)
        } else if raw_line.starts_with('-') {
            (format_line_number(old_line), String::new(), true, false)
        } else if raw_line.starts_with('\\') {
            (String::new(), String::new(), false, false)
        } else {
            (
                format_line_number(old_line),
                format_line_number(new_line),
                true,
                true,
            )
        };

        let content_style = git_line_style(raw_line, theme, is_error);
        rendered.push(Line::from(vec![
            Span::styled(format!("{old_label:>4}"), theme.muted_style()),
            Span::styled("│", theme.muted_style()),
            Span::styled(format!("{new_label:>4}"), theme.muted_style()),
            Span::styled("│ ", theme.muted_style()),
            Span::styled(raw_line.to_string(), content_style),
        ]));

        if advance_old {
            old_line = old_line.map(|line| line + 1);
        }
        if advance_new {
            new_line = new_line.map(|line| line + 1);
        }
    }

    rendered
}

fn format_line_number(line: Option<usize>) -> String {
    line.map(|line| line.to_string()).unwrap_or_default()
}

fn is_diff_metadata_line(line: &str) -> bool {
    line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
}

fn parse_unified_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_range, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new_range, _) = rest.split_once(' ')?;

    Some((parse_hunk_start(old_range)?, parse_hunk_start(new_range)?))
}

fn parse_hunk_start(range: &str) -> Option<usize> {
    range
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(range)
        .parse()
        .ok()
}
