use std::path::Path;

use imp_llm::truncate_chars_with_suffix;
use project_detect::{detect_walk, ProjectKind};
use serde::{Deserialize, Serialize};

use crate::guardrail_execution::execute;
use crate::process::ProcessManager;

#[path = "guardrails/guidance.rs"]
mod guidance;

const GUARDRAIL_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How strongly guardrail failures influence agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GuardrailLevel {
    /// Run checks and surface failures clearly, but do not block the turn.
    #[default]
    Advisory,
    /// Run checks and treat failures as blocking.
    Enforce,
}

/// Built-in guardrail starter profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuardrailProfile {
    /// Infer the profile from the current project using `project-detect`.
    Auto,
    /// Language-neutral fallback profile.
    Generic,
    /// Zig starter profile.
    Zig,
    /// Rust starter profile.
    Rust,
    /// TypeScript starter profile.
    #[serde(rename = "typescript")]
    TypeScript,
    /// C / C-family build-system starter profile.
    C,
    /// Go starter profile.
    Go,
    /// Elixir starter profile.
    Elixir,
    /// Kotlin starter profile.
    Kotlin,
}

impl GuardrailProfile {
    /// Concise prompt guidance for the agent, tailored to this profile.
    #[must_use]
    pub fn prompt_guidance(&self) -> &'static str {
        match self {
            Self::Auto => Self::Generic.prompt_guidance(),
            Self::Generic => guidance::GENERIC,
            Self::Zig => guidance::ZIG,
            Self::Rust => guidance::RUST,
            Self::TypeScript => guidance::TYPESCRIPT,
            Self::C => guidance::C,
            Self::Go => guidance::GO,
            Self::Elixir => guidance::ELIXIR,
            Self::Kotlin => guidance::KOTLIN,
        }
    }

    /// Default after-write check commands for this profile.
    #[must_use]
    pub fn default_after_write(&self) -> &'static [&'static str] {
        match self {
            Self::Auto | Self::Generic => &[],
            Self::Zig => &["zig fmt --check .", "zig build", "zig build test"],
            Self::Rust => &[
                "cargo fmt --check",
                "cargo clippy -- -D warnings",
                "cargo test",
            ],
            Self::TypeScript => &[],
            Self::C => &[],
            Self::Go => &["gofmt -l .", "go vet ./...", "go test ./..."],
            Self::Elixir => &[
                "mix format --check-formatted",
                "mix compile --warnings-as-errors",
                "mix test",
            ],
            Self::Kotlin => &[],
        }
    }

    /// Resolve a detected project kind to the nearest built-in profile.
    #[must_use]
    pub fn from_project_kind(kind: &ProjectKind) -> Self {
        match kind {
            ProjectKind::Zig => Self::Zig,
            ProjectKind::Cargo => Self::Rust,
            ProjectKind::Go => Self::Go,
            ProjectKind::Elixir { .. } => Self::Elixir,
            ProjectKind::Kotlin { .. } | ProjectKind::Gradle { .. } | ProjectKind::Maven => {
                Self::Kotlin
            }
            ProjectKind::Node { .. } => Self::TypeScript,
            ProjectKind::CMake | ProjectKind::Meson | ProjectKind::Make => Self::C,
            _ => Self::Generic,
        }
    }
}

/// Configurable engineering guardrails for agent-time guidance and checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailConfig {
    /// Master switch. `None` means "use the default".
    pub enabled: Option<bool>,
    /// Advisory vs blocking behavior.
    pub level: Option<GuardrailLevel>,
    /// Built-in profile selection.
    pub profile: Option<GuardrailProfile>,
    /// File globs that should trigger guardrail checks after writes.
    pub critical_paths: Option<Vec<String>>,
    /// Commands to run after writes. `None` means use profile defaults.
    pub after_write: Option<Vec<String>>,
}

impl GuardrailConfig {
    /// Returns whether guardrails are enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }

    /// Returns the effective configured level.
    #[must_use]
    pub fn effective_level(&self) -> GuardrailLevel {
        self.level.unwrap_or_default()
    }

    /// Returns the configured profile before auto-detection.
    #[must_use]
    pub fn configured_profile(&self) -> GuardrailProfile {
        self.profile.unwrap_or(GuardrailProfile::Generic)
    }

    /// Resolve the effective profile for a path.
    #[must_use]
    pub fn resolve_effective_profile(&self, cwd: &Path) -> GuardrailProfile {
        match self.configured_profile() {
            GuardrailProfile::Auto => detect_walk(cwd)
                .map(|(kind, _)| GuardrailProfile::from_project_kind(&kind))
                .unwrap_or(GuardrailProfile::Generic),
            profile => profile,
        }
    }

    /// Check whether a file path should trigger guardrail after-write checks.
    #[must_use]
    pub fn should_check_path(&self, path: &Path) -> bool {
        match &self.critical_paths {
            None => true,
            Some(patterns) if patterns.is_empty() => true,
            Some(patterns) => {
                let path_str = path.to_string_lossy();
                patterns.iter().any(|pat| {
                    glob::Pattern::new(pat)
                        .map(|g| g.matches(&path_str))
                        .unwrap_or(false)
                })
            }
        }
    }

    /// Merge another guardrail config into this one.
    pub fn merge(&mut self, other: GuardrailConfig) {
        if other.enabled.is_some() {
            self.enabled = other.enabled;
        }
        if other.level.is_some() {
            self.level = other.level;
        }
        if other.profile.is_some() {
            self.profile = other.profile;
        }
        if other.critical_paths.is_some() {
            self.critical_paths = other.critical_paths;
        }
        if other.after_write.is_some() {
            self.after_write = other.after_write;
        }
    }
}

/// Assemble the guardrails prompt layer for a resolved profile.
#[must_use]
pub fn guardrails_layer(profile: GuardrailProfile) -> String {
    let mut s = String::from("## Engineering Guardrails\n\n");
    s.push_str(profile.prompt_guidance());
    s
}

/// Result of running a single guardrail check command.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub command: String,
    pub success: bool,
    pub output: String,
}

/// Run guardrail after-write check commands and collect results.
pub async fn run_after_write_checks(
    config: &GuardrailConfig,
    effective_profile: GuardrailProfile,
    cwd: &Path,
) -> Vec<CheckResult> {
    let commands: Vec<String> = match &config.after_write {
        Some(cmds) if !cmds.is_empty() => cmds.clone(),
        _ => effective_profile
            .default_after_write()
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    };

    let manager = ProcessManager::new();
    let mut results = Vec::new();
    for cmd in &commands {
        let result = execute(&manager, cmd, cwd, GUARDRAIL_CHECK_TIMEOUT).await;

        match result {
            Ok(output) => {
                let combined = if output.stderr.is_empty() {
                    output.stdout
                } else {
                    format!("{}{}", output.stdout, output.stderr)
                };
                // Truncate to avoid flooding context
                let truncated = if combined.len() > 2000 {
                    format!(
                        "{}\n... (truncated)",
                        truncate_chars_with_suffix(&combined, 2000, "")
                    )
                } else {
                    combined
                };
                results.push(CheckResult {
                    command: cmd.clone(),
                    success: output.success,
                    output: truncated,
                });
            }
            Err(e) => {
                results.push(CheckResult {
                    command: cmd.clone(),
                    success: false,
                    output: format!("Failed to run: {e}"),
                });
            }
        }
    }
    results
}

/// Format check results into a message for the agent.
#[must_use]
pub fn format_check_results(results: &[CheckResult], level: GuardrailLevel) -> String {
    if results.is_empty() {
        return String::new();
    }

    let all_passed = results.iter().all(|r| r.success);
    if all_passed {
        return "Guardrail checks passed.".to_string();
    }

    let mut s = match level {
        GuardrailLevel::Enforce => {
            String::from("⚠ GUARDRAIL CHECK FAILED (enforce mode — fix before proceeding):\n")
        }
        GuardrailLevel::Advisory => {
            String::from("⚠ Guardrail check failed (advisory — review before continuing):\n")
        }
    };

    for r in results {
        if !r.success {
            s.push_str(&format!("\n  Command: {}\n", r.command));
            if !r.output.is_empty() {
                for line in r.output.lines().take(20) {
                    s.push_str(&format!("    {line}\n"));
                }
            }
        }
    }
    s
}

#[cfg(test)]
#[path = "guardrails_tests.rs"]
mod tests;
