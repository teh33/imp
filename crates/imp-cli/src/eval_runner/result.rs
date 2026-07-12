use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::spec::EvalTaskSpec;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct EvalRunResult {
    pub schema_version: u32,
    pub run_id: String,
    pub task: String,
    pub status: EvalRunStatus,
    pub source: EvalSource,
    pub checkout: PathBuf,
    pub agent: EvalAgentResult,
    pub verifier: EvalVerifierResult,
    #[serde(default)]
    pub expectations: EvalExpectationResult,
    pub diff: EvalDiffSummary,
    pub artifacts: EvalArtifacts,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvalRunStatus {
    Prepared,
    Passed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvalSource {
    pub repo: String,
    pub commit: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct EvalAgentResult {
    #[serde(default)]
    pub kind: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: Option<u64>,
    pub outcome: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvalVerifierResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: Option<u64>,
    pub passed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvalExpectationResult {
    pub passed: bool,
    pub failures: Vec<String>,
}

impl Default for EvalExpectationResult {
    fn default() -> Self {
        Self {
            passed: true,
            failures: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvalDiffSummary {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EvalArtifacts {
    pub prompt: String,
    pub agent_stdout: String,
    pub agent_stderr: String,
    pub diff: String,
    pub verifier: String,
    pub result: String,
}

impl EvalRunResult {
    pub(crate) fn command_succeeded(&self) -> bool {
        matches!(self.status, EvalRunStatus::Prepared | EvalRunStatus::Passed)
    }

    pub(super) fn initial(run_id: String, spec: &EvalTaskSpec, checkout: PathBuf) -> Self {
        Self {
            schema_version: 1,
            run_id,
            task: spec.id.clone(),
            status: EvalRunStatus::Blocked,
            source: EvalSource {
                repo: spec
                    .fixture
                    .as_ref()
                    .map(|path| format!("fixture:{}", path.display()))
                    .unwrap_or_else(|| spec.repo.clone()),
                commit: spec.commit.clone(),
            },
            checkout,
            agent: EvalAgentResult::default(),
            verifier: EvalVerifierResult {
                command: spec.verifier.clone(),
                exit_code: None,
                timed_out: false,
                duration_ms: None,
                passed: None,
            },
            expectations: EvalExpectationResult {
                passed: true,
                failures: Vec::new(),
            },
            diff: EvalDiffSummary::default(),
            artifacts: EvalArtifacts {
                prompt: "prompt.md".to_string(),
                agent_stdout: "agent.stdout.json".to_string(),
                agent_stderr: "agent.stderr.txt".to_string(),
                diff: "diff.patch".to_string(),
                verifier: "verifier.txt".to_string(),
                result: "result.json".to_string(),
            },
            error: None,
        }
    }
}

pub(crate) fn load_result(path: &Path) -> Result<EvalRunResult, Box<dyn std::error::Error>> {
    let path = if path.is_dir() {
        path.join("result.json")
    } else {
        path.to_path_buf()
    };
    let json = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read eval result {}: {error}", path.display()))?;
    serde_json::from_str(&json)
        .map_err(|error| format!("failed to parse eval result {}: {error}", path.display()).into())
}

pub(crate) fn compare_results(
    baseline: &EvalRunResult,
    candidate: &EvalRunResult,
) -> Result<Value, Box<dyn std::error::Error>> {
    if baseline.task != candidate.task {
        return Err(format!(
            "cannot compare different eval tasks: baseline is `{}`, candidate is `{}`",
            baseline.task, candidate.task
        )
        .into());
    }
    Ok(serde_json::json!({
        "schema_version": 1,
        "task": candidate.task,
        "baseline": {
            "run_id": baseline.run_id,
            "status": baseline.status,
            "agent_duration_ms": baseline.agent.duration_ms,
            "verifier_duration_ms": baseline.verifier.duration_ms,
            "files_changed": baseline.diff.files_changed,
        },
        "candidate": {
            "run_id": candidate.run_id,
            "status": candidate.status,
            "agent_duration_ms": candidate.agent.duration_ms,
            "verifier_duration_ms": candidate.verifier.duration_ms,
            "files_changed": candidate.diff.files_changed,
        },
        "delta": {
            "agent_duration_ms": signed_delta(candidate.agent.duration_ms, baseline.agent.duration_ms),
            "verifier_duration_ms": signed_delta(candidate.verifier.duration_ms, baseline.verifier.duration_ms),
            "files_changed": i64::from(candidate.diff.files_changed) - i64::from(baseline.diff.files_changed),
        },
        "regression": baseline.status == EvalRunStatus::Passed && candidate.status != EvalRunStatus::Passed,
        "improvement": baseline.status != EvalRunStatus::Passed && candidate.status == EvalRunStatus::Passed,
    }))
}

pub(super) fn write_result(
    output_dir: &Path,
    result: &EvalRunResult,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(result)?;
    fs::write(output_dir.join("result.json"), format!("{json}\n"))?;
    Ok(())
}

fn signed_delta(candidate: Option<u64>, baseline: Option<u64>) -> Option<i64> {
    Some(i64::try_from(candidate?).ok()? - i64::try_from(baseline?).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(run_id: &str, task: &str, status: EvalRunStatus) -> EvalRunResult {
        let spec = EvalTaskSpec {
            id: task.to_string(),
            repo: "repo".into(),
            commit: "a".repeat(40),
            prompt: "prompt".into(),
            verifier: "check".into(),
            fixture: None,
            expectations: Default::default(),
            setup: None,
            max_turns: None,
            timeout_seconds: None,
            verifier_timeout_seconds: None,
        };
        let mut result = EvalRunResult::initial(run_id.into(), &spec, "checkout".into());
        result.status = status;
        result
    }

    #[test]
    fn comparison_reports_pass_regression() {
        let baseline = result("base", "task", EvalRunStatus::Passed);
        let candidate = result("candidate", "task", EvalRunStatus::Failed);

        let comparison = compare_results(&baseline, &candidate).unwrap();

        assert_eq!(comparison["regression"], true);
        assert_eq!(comparison["improvement"], false);
    }

    #[test]
    fn comparison_rejects_different_tasks() {
        let baseline = result("base", "one", EvalRunStatus::Passed);
        let candidate = result("candidate", "two", EvalRunStatus::Passed);

        assert!(compare_results(&baseline, &candidate).is_err());
    }
}
