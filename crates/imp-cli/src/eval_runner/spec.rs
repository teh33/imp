use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EvalTaskSpec {
    pub id: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub commit: String,
    pub prompt: String,
    pub verifier: String,
    #[serde(default)]
    pub fixture: Option<PathBuf>,
    #[serde(default)]
    pub expectations: EvalExpectations,
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub verifier_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct EvalExpectations {
    #[serde(default)]
    pub require_changes: Option<bool>,
    #[serde(default)]
    pub allowed_changed_paths: Vec<String>,
    #[serde(default)]
    pub required_changed_paths: Vec<String>,
    #[serde(default)]
    pub max_files_changed: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpecValidation {
    pub task: String,
    pub path: PathBuf,
    pub valid: bool,
    pub errors: Vec<String>,
}

impl EvalTaskSpec {
    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)
            .map_err(|error| format!("failed to read eval task {}: {error}", path.display()))?;
        serde_json::from_str(&content).map_err(|error| {
            format!("failed to parse eval task {}: {error}", path.display()).into()
        })
    }

    pub(crate) fn execution_prompt(&self) -> String {
        format!(
            "{}\n\nRequired verification command: `{}`",
            self.prompt.trim(),
            self.verifier.trim()
        )
    }

    pub(crate) fn validate(&self, path: &Path) -> SpecValidation {
        let mut errors = Vec::new();
        if self.id.trim().is_empty() {
            errors.push("id must not be empty".to_string());
        } else if !self
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            || matches!(self.id.as_str(), "." | "..")
        {
            errors.push(
                "id may contain only ASCII letters, digits, '.', '-', and '_' and must not be '.' or '..'"
                    .to_string(),
            );
        }
        if path.file_stem().and_then(|value| value.to_str()) != Some(self.id.as_str()) {
            errors.push(format!(
                "id `{}` must match file name `{}`",
                self.id,
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("<invalid>")
            ));
        }
        if let Some(fixture) = &self.fixture {
            if !self.repo.trim().is_empty() || !self.commit.trim().is_empty() {
                errors.push("fixture tasks must not also set repo or commit".to_string());
            }
            let fixture = resolve_relative_to_spec(path, fixture);
            if !fixture.is_dir() {
                errors.push(format!(
                    "fixture directory does not exist: {}",
                    fixture.display()
                ));
            }
        } else {
            if self.repo.trim().is_empty() {
                errors.push("repo must not be empty".to_string());
            }
            if self.commit.len() != 40 || !self.commit.chars().all(|ch| ch.is_ascii_hexdigit()) {
                errors.push("commit must be a pinned 40-character hexadecimal SHA".to_string());
            }
        }
        if self.prompt.trim().is_empty() {
            errors.push("prompt must not be empty".to_string());
        }
        if self.verifier.trim().is_empty() {
            errors.push("verifier must not be empty".to_string());
        } else if self.verifier.to_ascii_lowercase().contains("tbd") {
            errors.push("verifier is unresolved (contains TBD)".to_string());
        }

        if self
            .setup
            .as_deref()
            .is_some_and(|setup| setup.trim().is_empty())
        {
            errors.push("setup must not be empty when provided".to_string());
        }
        if self.max_turns == Some(0) {
            errors.push("max_turns must be greater than zero".to_string());
        }
        if self.timeout_seconds == Some(0) || self.verifier_timeout_seconds == Some(0) {
            errors.push("timeouts must be greater than zero".to_string());
        }

        validate_expectations(&self.expectations, &mut errors);

        SpecValidation {
            task: self.id.clone(),
            path: path.to_path_buf(),
            valid: errors.is_empty(),
            errors,
        }
    }
}

pub(crate) fn task_paths(suite: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = fs::read_dir(suite)
        .map_err(|error| format!("failed to read eval suite {}: {error}", suite.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub(crate) fn resolve_task_path(suite: &Path, task: &str) -> PathBuf {
    let path = Path::new(task);
    if path.extension().is_some() || path.components().count() > 1 {
        path.to_path_buf()
    } else {
        suite.join(format!("{task}.json"))
    }
}

pub(crate) fn resolve_relative_to_spec(spec_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        spec_path.parent().unwrap_or(Path::new(".")).join(path)
    }
}

fn validate_expectations(expectations: &EvalExpectations, errors: &mut Vec<String>) {
    for path in expectations
        .allowed_changed_paths
        .iter()
        .chain(&expectations.required_changed_paths)
    {
        let candidate = Path::new(path);
        if candidate.is_absolute()
            || candidate
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            errors.push(format!(
                "expected changed path must be repository-relative: {path}"
            ));
        }
    }
    if expectations.max_files_changed == Some(0) && expectations.require_changes == Some(true) {
        errors.push("require_changes=true conflicts with max_files_changed=0".to_string());
    }
}

#[cfg(test)]
mod tests;
