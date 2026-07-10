use imp_llm::truncate_chars_with_suffix;

use super::super::ToolOutput;
use crate::tools::fuzzy;

pub(crate) fn apply_edit(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> std::result::Result<(String, bool), ToolOutput> {
    if let Some(position) = content.find(old_text) {
        return Ok((
            replace_range(content, position, position + old_text.len(), new_text),
            false,
        ));
    }
    if let Some(matched) = fuzzy::fuzzy_find(content, old_text) {
        return Ok((
            replace_range(content, matched.start, matched.end, new_text),
            true,
        ));
    }
    let preview = truncate_chars_with_suffix(content, 200, "");
    Err(ToolOutput::error(format!(
        "Could not find the specified text to replace.\nFirst 200 chars of file:\n{preview}"
    )))
}

fn replace_range(content: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut result = String::with_capacity(content.len());
    result.push_str(&content[..start]);
    result.push_str(replacement);
    result.push_str(&content[end..]);
    result
}
