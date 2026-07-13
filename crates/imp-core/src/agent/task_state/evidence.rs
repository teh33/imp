use std::path::Path;

use imp_llm::ToolResultMessage;

use super::{CheckRecord, SessionTaskState};

impl SessionTaskState {
    pub fn record_tool_result(&mut self, result: &ToolResultMessage, cwd: &Path) {
        if !self.is_enabled() {
            return;
        }
        match result.tool_name.as_str() {
            "write" | "edit" | "multi_edit" if !result.is_error => {
                if result.details["dry_run"].as_bool() == Some(true) {
                    return;
                }
                self.record_changed_paths(&result.details, cwd);
            }
            "bash" => self.record_bash(result),
            _ => {}
        }
    }

    fn record_changed_paths(&mut self, details: &serde_json::Value, cwd: &Path) {
        for path in detail_paths(details) {
            self.changed_paths.insert(display_path(cwd, path));
        }
        if !self.changed_paths.is_empty() {
            self.verification_required = true;
            if self.changed_paths.len() >= 3 {
                self.activate_planning();
            }
            self.failures
                .retain(|failure| failure != "verification required after file changes");
        }
    }

    fn record_bash(&mut self, result: &ToolResultMessage) {
        if result.details["managed_job"].as_bool() == Some(true) {
            return;
        }
        let command = result.details["command"]
            .as_str()
            .unwrap_or("<unknown command>")
            .to_string();
        let exit_code = result.details["exit_code"].as_i64();
        let passed = !result.is_error && exit_code == Some(0);
        self.checks.push(CheckRecord {
            command: command.clone(),
            passed,
            exit_code,
        });
        let failure = format!("command failed: {command}");
        if passed {
            self.failures.retain(|existing| {
                existing != &failure
                    && existing != "command failed: <unknown command>"
                    && !(command_looks_like_check(&command)
                        && failed_command(existing).is_some_and(command_looks_like_check))
            });
            if self.verification_required && command_looks_like_check(&command) {
                self.verification_required = false;
            }
        } else if !self.failures.contains(&failure) {
            self.failures.push(failure);
            if self.failures.len() >= 2 {
                self.activate_planning();
            }
        }
    }
}

fn failed_command(failure: &str) -> Option<&str> {
    failure.strip_prefix("command failed: ")
}

fn command_looks_like_check(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    [
        " test",
        "test ",
        "pytest",
        "check",
        "verify",
        "lint",
        "clippy",
        "cargo fmt",
    ]
    .iter()
    .any(|needle| command.contains(needle))
}

fn detail_paths(details: &serde_json::Value) -> Vec<&str> {
    if let Some(files) = details["files"].as_array() {
        return files
            .iter()
            .filter_map(|file| file["path"].as_str())
            .collect();
    }
    details["path"].as_str().into_iter().collect()
}

fn display_path(cwd: &Path, path: &str) -> String {
    Path::new(path)
        .strip_prefix(cwd)
        .unwrap_or(Path::new(path))
        .display()
        .to_string()
}
