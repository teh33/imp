use std::path::{Path, PathBuf};

use serde_json::json;

use crate::tools::{truncate_head, ToolOutput};

const DISPLAY_MAX_LINES: usize = 400;
const DISPLAY_MAX_BYTES: usize = 32 * 1024;

pub(super) fn stdout_lossy(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace('\r', "")
}

pub(super) fn stderr_lossy(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace('\r', "")
}

pub(super) fn stdout_trimmed(output: &std::process::Output) -> String {
    stdout_lossy(output).trim().to_string()
}

pub(super) fn stderr_trimmed(output: &std::process::Output) -> String {
    stderr_lossy(output).trim().to_string()
}

pub(super) fn not_git_repo_message(cwd: &Path, output: &std::process::Output) -> String {
    let stderr = stderr_trimmed(output);
    if stderr.is_empty() {
        format!("Not inside a git repository: {}", cwd.display())
    } else {
        format!("Not inside a git repository: {}\n{}", cwd.display(), stderr)
    }
}

pub(super) fn git_failure(prefix: &str, output: &std::process::Output) -> ToolOutput {
    let stdout = stdout_trimmed(output);
    let stderr = stderr_trimmed(output);
    let combined = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => prefix.to_string(),
        (false, true) => format!("{prefix}: {stdout}"),
        (true, false) => format!("{prefix}: {stderr}"),
        (false, false) => format!("{prefix}: {stdout}\n{stderr}"),
    };
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: combined }],
        details: json!({
            "success": false,
            "exit_code": output.status.code(),
            "stdout": stdout,
            "stderr": stderr,
        }),
        is_error: true,
    }
}

pub(super) fn display_or_unknown(s: &str) -> &str {
    if s.trim().is_empty() {
        "unknown"
    } else {
        s
    }
}

pub(super) fn truncate_for_display(text: &str) -> (String, String, Option<PathBuf>) {
    let truncated = truncate_head(text, DISPLAY_MAX_LINES, DISPLAY_MAX_BYTES);
    let content = truncated.content.trim_end().to_string();
    let note = if truncated.truncated {
        let base = format!(
            "[output truncated: showing {}/{} lines, {}/{} bytes]",
            truncated.output_lines,
            truncated.total_lines,
            truncated.output_bytes,
            truncated.total_bytes,
        );
        match &truncated.temp_file {
            Some(path) => format!("{base} full output: {}", path.display()),
            None => base,
        }
    } else {
        String::new()
    };
    (content, note, truncated.temp_file)
}
