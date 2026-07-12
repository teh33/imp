use glob::{MatchOptions, Pattern};

use std::time::{SystemTime, UNIX_EPOCH};

use super::process::ProcessOutcome;
use super::result::{EvalAgentResult, EvalDiffSummary, EvalExpectationResult};
use super::spec::EvalTaskSpec;

pub(super) async fn run_shell_to_file(
    command: &str,
    cwd: &std::path::Path,
    path: &std::path::Path,
    timeout: std::time::Duration,
) -> Result<ProcessOutcome, Box<dyn std::error::Error>> {
    let args = vec!["-lc".to_string(), command.to_string()];
    super::process::run_logged(std::path::Path::new("sh"), &args, cwd, path, path, timeout).await
}

pub(super) fn evaluate_expectations(
    spec: &EvalTaskSpec,
    diff: &EvalDiffSummary,
) -> EvalExpectationResult {
    let mut failures = Vec::new();
    validate_change_count(spec, diff, &mut failures);
    validate_required_paths(spec, diff, &mut failures);
    validate_allowed_paths(spec, diff, &mut failures);
    EvalExpectationResult {
        passed: failures.is_empty(),
        failures,
    }
}

fn validate_change_count(spec: &EvalTaskSpec, diff: &EvalDiffSummary, failures: &mut Vec<String>) {
    if spec.expectations.require_changes == Some(true) && diff.files_changed == 0 {
        failures.push("expected at least one changed file".to_string());
    }
    if spec.expectations.require_changes == Some(false) && diff.files_changed > 0 {
        failures.push(format!(
            "expected no changed files, found {}",
            diff.files_changed
        ));
    }
    if let Some(max) = spec.expectations.max_files_changed {
        if diff.files_changed > max {
            failures.push(format!(
                "expected at most {max} changed file(s), found {}",
                diff.files_changed
            ));
        }
    }
}

fn validate_required_paths(
    spec: &EvalTaskSpec,
    diff: &EvalDiffSummary,
    failures: &mut Vec<String>,
) {
    for required in &spec.expectations.required_changed_paths {
        if !diff.paths.iter().any(|path| path_matches(path, required)) {
            failures.push(format!("required changed path missing: {required}"));
        }
    }
}

fn validate_allowed_paths(spec: &EvalTaskSpec, diff: &EvalDiffSummary, failures: &mut Vec<String>) {
    if spec.expectations.allowed_changed_paths.is_empty() {
        return;
    }
    for path in &diff.paths {
        if !spec
            .expectations
            .allowed_changed_paths
            .iter()
            .any(|allowed| path_matches(path, allowed))
        {
            failures.push(format!("changed path is outside the allowlist: {path}"));
        }
    }
}

fn path_matches(path: &str, pattern: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = pattern.trim().replace('\\', "/");
    pattern == path
        || Pattern::new(&pattern).is_ok_and(|pattern| {
            pattern.matches_with(
                &path,
                MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: true,
                },
            )
        })
}

pub(super) fn expand_verifier(command: &str, paths: &[String]) -> String {
    let quoted = paths
        .iter()
        .map(|path| shell_quote(path))
        .collect::<Vec<_>>();
    command.replace("<modified-files>", &quoted.join(" "))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn run_id(task: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{millis}-{task}-{}", uuid::Uuid::new_v4().simple())
}

pub(super) fn format_agent_failure(result: &EvalAgentResult, outcome: ProcessOutcome) -> String {
    if outcome.timed_out {
        return format!("agent timed out after {} ms", outcome.duration_ms);
    }
    if outcome.exit_code != Some(0) {
        return format!("agent exited {:?}", outcome.exit_code);
    }
    if let Some(error) = result
        .outcome
        .as_ref()
        .and_then(|value| value["error"].as_str())
    {
        return format!("agent reported error: {error}");
    }
    "agent did not produce a successful completion event".to_string()
}

pub(super) fn format_process_failure(label: &str, outcome: ProcessOutcome) -> String {
    if outcome.timed_out {
        format!("{label} timed out after {} ms", outcome.duration_ms)
    } else {
        format!("{label} exited {:?}", outcome.exit_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expectation_globs_match_nested_paths() {
        let spec = EvalTaskSpec {
            expectations: super::super::spec::EvalExpectations {
                require_changes: Some(true),
                allowed_changed_paths: vec!["src/**/*.rs".into(), "tests/**".into()],
                required_changed_paths: vec!["src/**/*.rs".into()],
                max_files_changed: Some(2),
            },
            ..test_spec()
        };
        let diff = EvalDiffSummary {
            files_changed: 2,
            paths: vec!["src/parser/mod.rs".into(), "tests/parser.rs".into()],
            ..Default::default()
        };

        let result = evaluate_expectations(&spec, &diff);

        assert!(result.passed, "{:?}", result.failures);
    }

    #[test]
    fn expectation_globs_reject_paths_outside_allowlist() {
        let spec = EvalTaskSpec {
            expectations: super::super::spec::EvalExpectations {
                allowed_changed_paths: vec!["src/**/*.rs".into()],
                ..Default::default()
            },
            ..test_spec()
        };
        let diff = EvalDiffSummary {
            files_changed: 1,
            paths: vec!["docs/guide.md".into()],
            ..Default::default()
        };

        let result = evaluate_expectations(&spec, &diff);

        assert!(!result.passed);
        assert!(result.failures[0].contains("outside the allowlist"));
    }

    fn test_spec() -> EvalTaskSpec {
        EvalTaskSpec {
            id: "test".into(),
            repo: "https://example.test/repo.git".into(),
            commit: "a".repeat(40),
            prompt: "prompt".into(),
            verifier: "true".into(),
            fixture: None,
            expectations: Default::default(),
            setup: None,
            max_turns: None,
            timeout_seconds: None,
            verifier_timeout_seconds: None,
        }
    }

    #[test]
    fn normalized_agent_error_is_reported_when_process_exits_zero() {
        let result = EvalAgentResult {
            outcome: Some(serde_json::json!({"error": "No API key"})),
            ..Default::default()
        };
        let message = format_agent_failure(
            &result,
            ProcessOutcome {
                exit_code: Some(0),
                timed_out: false,
                duration_ms: 1,
            },
        );
        assert_eq!(message, "agent reported error: No API key");
    }

    #[test]
    fn verifier_expansion_shell_quotes_modified_paths() {
        let command = expand_verifier(
            "ruff check <modified-files>",
            &["src/a.py".into(), "path with space/b.py".into()],
        );
        assert_eq!(command, "ruff check 'src/a.py' 'path with space/b.py'");
    }
}
