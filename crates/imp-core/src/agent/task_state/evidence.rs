use std::collections::BTreeSet;
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
                existing != "command failed: <unknown command>"
                    && !failed_command(existing)
                        .is_some_and(|failed| commands_are_related(failed, &command))
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
    pub(super) fn unresolved_failures(&self) -> Vec<String> {
        self.failures
            .iter()
            .filter(|failure| !self.failure_was_resolved(failure))
            .cloned()
            .collect()
    }

    fn failure_was_resolved(&self, failure: &str) -> bool {
        let Some(failed) = failed_command(failure) else {
            return false;
        };
        let failed_at = self
            .checks
            .iter()
            .rposition(|check| !check.passed && check.command == failed);
        self.checks.iter().enumerate().any(|(index, check)| {
            check.passed
                && failed_at.is_none_or(|failed_at| index > failed_at)
                && (failed == "<unknown command>" || commands_are_related(failed, &check.command))
        })
    }
}

fn failed_command(failure: &str) -> Option<&str> {
    failure.strip_prefix("command failed: ")
}

fn commands_are_related(failed: &str, passed: &str) -> bool {
    if failed == passed || (command_looks_like_check(failed) && command_looks_like_check(passed)) {
        return true;
    }
    let failed = command_subject_tokens(failed);
    let passed = command_subject_tokens(passed);
    failed.intersection(&passed).take(3).count() >= 3
}

fn command_subject_tokens(command: &str) -> BTreeSet<String> {
    command
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() >= 4 && !is_shell_noise(token))
        .collect()
}

fn is_shell_noise(token: &str) -> bool {
    matches!(
        token,
        "then" | "else" | "true" | "false" | "echo" | "printf" | "set" | "pipefail"
    )
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

#[cfg(test)]
mod tests {
    use imp_llm::{ContentBlock, ToolResultMessage};

    use super::*;

    fn bash(command: &str, passed: bool) -> ToolResultMessage {
        ToolResultMessage {
            tool_call_id: "call".into(),
            tool_name: "bash".into(),
            content: vec![ContentBlock::Text {
                text: String::new(),
            }],
            is_error: !passed,
            details: serde_json::json!({
                "command": command,
                "exit_code": if passed { 0 } else { 1 },
            }),
            timestamp: 0,
        }
    }

    #[test]
    fn corrected_diagnostic_command_resolves_related_failure() {
        let failed = "rg cache_write_per_mtok model.rs missing/models.rs openai.rs";
        let passed = "rg cache_write_per_mtok model.rs openai.rs";
        let mut state = SessionTaskState::new("Inspect cache accounting");

        state.record_tool_result(&bash(failed, false), Path::new("/repo"));
        state.record_tool_result(&bash(passed, true), Path::new("/repo"));

        assert!(state.unresolved_failures().is_empty());
    }

    #[test]
    fn restored_checks_reconcile_stale_failure_for_closeout_and_projection() {
        let failed = "curl prompt-caching token-counting BeautifulSoup";
        let passed = "curl prompt-caching token-counting HTMLParser";
        let mut state = SessionTaskState::new("Read provider docs");
        state.failures = vec![format!("command failed: {failed}")];
        state.checks = vec![
            CheckRecord {
                command: failed.into(),
                passed: false,
                exit_code: Some(1),
            },
            CheckRecord {
                command: passed.into(),
                passed: true,
                exit_code: Some(0),
            },
        ];

        assert!(state.closeout_issues().is_empty());
        assert!(!state.projection().contains("Unresolved failures"));
    }

    #[test]
    fn later_failed_retry_remains_unresolved() {
        let command = "ready apply cache-readiness json dry-run";
        let mut state = SessionTaskState::new("Update readiness");
        state.checks = vec![
            CheckRecord {
                command: command.into(),
                passed: true,
                exit_code: Some(0),
            },
            CheckRecord {
                command: command.into(),
                passed: false,
                exit_code: Some(1),
            },
        ];
        state.failures = vec![format!("command failed: {command}")];

        assert_eq!(state.unresolved_failures(), state.failures);
    }

    #[test]
    fn corrected_diagnostic_subjects_are_related() {
        let pairs = [
            (
                "curl developers.openai.com prompt-caching token-counting BeautifulSoup",
                "curl developers.openai.com prompt-caching token-counting HTMLParser",
            ),
            (
                "rg gpt-5.6 cache_write_per_mtok model.rs openai/models.rs openai.rs",
                "rg gpt-5.6 cache_write_per_mtok model.rs openai.rs",
            ),
            (
                "ready apply imp-cache-readiness.yaml ready show crit_cache crit_tokens",
                "ready apply imp-cache-readiness.json dry-run ready show crit_cache crit_tokens",
            ),
            (
                "ready apply stdin json dry-run expected_revision empty changes",
                "ready apply input json dry-run expected_revision assessed changes",
            ),
            (
                "rg cargo install imp-install README crates scripts Makefile justfile",
                "rg cargo install imp-install README Cargo.toml crates src",
            ),
        ];

        for (failed, passed) in pairs {
            assert!(commands_are_related(failed, passed), "{failed}");
        }
    }

    #[test]
    fn unrelated_diagnostic_subjects_remain_unrelated() {
        let pairs = [
            ("git show missing commit", "cargo test task state"),
            ("curl provider docs anthropic", "rg provider code anthropic"),
            ("ready apply cache ledger", "cargo check ready crate"),
        ];

        for (failed, passed) in pairs {
            assert!(!commands_are_related(failed, passed), "{failed}");
        }
    }

    #[test]
    fn two_generic_tokens_do_not_make_commands_related() {
        assert!(!commands_are_related(
            "rg source model missing.rs",
            "cat source model output.txt",
        ));
    }
}
