use std::path::Path;
use std::process::Stdio;

use imp_llm::truncate_chars_with_suffix;
use project_detect::{detect_walk, ProjectKind};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::child_process::{isolate_tokio_command, kill_tokio_process_group};

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
            Self::Generic => GUIDANCE_GENERIC,
            Self::Zig => GUIDANCE_ZIG,
            Self::Rust => GUIDANCE_RUST,
            Self::TypeScript => GUIDANCE_TYPESCRIPT,
            Self::C => GUIDANCE_C,
            Self::Go => GUIDANCE_GO,
            Self::Elixir => GUIDANCE_ELIXIR,
            Self::Kotlin => GUIDANCE_KOTLIN,
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

    let mut results = Vec::new();
    for cmd in &commands {
        let result = run_guardrail_command(cmd, cwd, GUARDRAIL_CHECK_TIMEOUT).await;

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = if stderr.is_empty() {
                    stdout.to_string()
                } else {
                    format!("{stdout}{stderr}")
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
                    success: output.status.success(),
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

async fn run_guardrail_command(
    cmd: &str,
    cwd: &Path,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    isolate_tokio_command(&mut command);

    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status_result) => {
            let status = status_result?;
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            if let Some(mut stream) = stdout.take() {
                stream.read_to_end(&mut stdout_bytes).await?;
            }
            if let Some(mut stream) = stderr.take() {
                stream.read_to_end(&mut stderr_bytes).await?;
            }
            Ok(std::process::Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            })
        }
        Err(_) => {
            kill_tokio_process_group(&child).await;
            let _ = child.kill().await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("guardrail command timed out after {}s", timeout.as_secs()),
            ))
        }
    }
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

// -- Prompt guidance text per profile ----------------------------------------

const GUIDANCE_GENERIC: &str = "\
- Prefer the smallest, local fix over a cross-file refactor.
- Search for existing patterns first; mirror naming, error handling, and conventions.
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Make error handling explicit — don't silently ignore failures.
- Leave code warning-free and easy to verify.
- Don't add new dependencies without explicit user approval.
";

const GUIDANCE_ZIG: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and buffers bounded.
- Handle errors explicitly with try/catch — avoid casual catch unreachable.
- Keep allocator ownership and lifetime clear.
- Prefer small, readable functions with minimal hidden control flow.
- Leave code formatted, buildable, and warning-free.
";

const GUIDANCE_RUST: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Use Result with meaningful error propagation — avoid unwrap() in non-test code.
- Keep async behavior bounded and timeouts explicit.
- Prefer small, focused changes over broad rewrites.
- Leave code clippy-clean with zero warnings.
";

const GUIDANCE_TYPESCRIPT: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Make error handling explicit — don't silently swallow rejections or errors.
- Use strict typing — avoid any unless justified.
- Keep async/Promise flows bounded and understandable.
- Leave typecheck and lint status clean.
";

const GUIDANCE_C: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and buffer sizes bounded.
- Make error handling explicit — check return values.
- Keep pointer usage straightforward and well-scoped.
- Avoid preprocessor complexity when simpler code works.
- Leave build and test status clean.
";

const GUIDANCE_GO: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep loops, retries, and timeouts bounded.
- Check and propagate errors explicitly — don't ignore returned errors.
- Keep goroutine lifecycle and cancellation understandable.
- Prefer small functions and direct control flow.
- Leave formatting and vet status clean.
";

const GUIDANCE_ELIXIR: &str = "\
- Keep control flow straightforward and easy to follow.
- Keep retries and message flows bounded.
- Keep process and supervision boundaries clear.
- Handle {:ok, value} / {:error, reason} tuples explicitly.
- Avoid hiding important behavior in opaque control flow.
- Leave formatting and compilation warnings-free.
";

const GUIDANCE_KOTLIN: &str = "\
- Keep control flow straightforward and easy to follow.
- Prefer val over var; keep mutation local and obvious.
- Treat nullability as part of the design — avoid !! outside tests or impossible states.
- Use structured concurrency; avoid GlobalScope and do not swallow CancellationException.
- Keep Gradle/Maven verification project-specific and use ./gradlew when available.
- Leave formatting, lint, and tests clean for the touched module.
";

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tempfile::TempDir;

    #[derive(Debug, Deserialize)]
    struct GuardrailToml {
        guardrails: GuardrailConfig,
    }

    #[tokio::test]
    async fn guardrail_command_timeout_returns_error() {
        let dir = TempDir::new().unwrap();
        let started = std::time::Instant::now();

        let result =
            run_guardrail_command("sleep 5", dir.path(), std::time::Duration::from_millis(50))
                .await;

        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            result,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn guardrail_toml_deserializes() {
        let parsed: GuardrailToml = toml::from_str(
            r#"
[guardrails]
enabled = true
level = "enforce"
profile = "zig"
critical_paths = ["src/**", "lib/**"]
after_write = ["zig fmt --check .", "zig build"]
"#,
        )
        .unwrap();

        assert_eq!(parsed.guardrails.enabled, Some(true));
        assert_eq!(parsed.guardrails.level, Some(GuardrailLevel::Enforce));
        assert_eq!(parsed.guardrails.profile, Some(GuardrailProfile::Zig));
        assert_eq!(
            parsed.guardrails.critical_paths,
            Some(vec!["src/**".into(), "lib/**".into()])
        );
        assert_eq!(
            parsed.guardrails.after_write,
            Some(vec!["zig fmt --check .".into(), "zig build".into()])
        );
    }

    #[test]
    fn guardrail_auto_profile_resolves_zig() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("build.zig"), "").unwrap();

        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(dir.path()),
            GuardrailProfile::Zig
        );
    }

    #[test]
    fn guardrail_auto_profile_resolves_rust_from_subdirectory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.1.0'\n",
        )
        .unwrap();
        let nested = dir.path().join("src").join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(&nested),
            GuardrailProfile::Rust
        );
    }

    #[test]
    fn guardrail_auto_profile_resolves_go() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/test\n").unwrap();

        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(dir.path()),
            GuardrailProfile::Go
        );
    }

    #[test]
    fn guardrail_auto_profile_resolves_elixir() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mix.exs"),
            "defmodule Demo.MixProject do end\n",
        )
        .unwrap();

        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(dir.path()),
            GuardrailProfile::Elixir
        );
    }

    #[test]
    fn guardrail_auto_profile_resolves_kotlin_gradle() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("settings.gradle.kts"),
            "pluginManagement {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("build.gradle.kts"),
            "plugins { kotlin(\"jvm\") version \"2.0.0\" }\n",
        )
        .unwrap();

        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(dir.path()),
            GuardrailProfile::Kotlin
        );
    }

    #[test]
    fn guardrail_auto_profile_falls_back_to_generic() {
        let dir = TempDir::new().unwrap();
        let config = GuardrailConfig {
            profile: Some(GuardrailProfile::Auto),
            ..Default::default()
        };

        assert_eq!(
            config.resolve_effective_profile(dir.path()),
            GuardrailProfile::Generic
        );
    }

    #[test]
    fn guardrail_prompt_guidance_varies_by_profile() {
        let zig = GuardrailProfile::Zig.prompt_guidance();
        let rust = GuardrailProfile::Rust.prompt_guidance();
        let generic = GuardrailProfile::Generic.prompt_guidance();

        assert!(zig.contains("catch unreachable"));
        assert!(zig.contains("allocator"));
        assert!(rust.contains("clippy"));
        assert!(rust.contains("unwrap"));
        assert!(generic.contains("warning-free"));
        assert_ne!(zig, rust);
        assert_ne!(zig, generic);
    }

    #[test]
    fn guardrail_default_after_write_zig() {
        let cmds = GuardrailProfile::Zig.default_after_write();
        assert_eq!(cmds.len(), 3);
        assert!(cmds[0].contains("zig fmt"));
    }

    #[test]
    fn guardrail_default_after_write_generic_is_empty() {
        assert!(GuardrailProfile::Generic.default_after_write().is_empty());
    }

    #[test]
    fn guardrail_layer_contains_header() {
        let layer = guardrails_layer(GuardrailProfile::Zig);
        assert!(layer.starts_with("## Engineering Guardrails"));
        assert!(layer.contains("catch unreachable"));
    }

    #[test]
    fn guardrail_format_check_results_all_passed() {
        let results = vec![CheckResult {
            command: "zig build".into(),
            success: true,
            output: String::new(),
        }];
        let msg = format_check_results(&results, GuardrailLevel::Advisory);
        assert_eq!(msg, "Guardrail checks passed.");
    }

    #[test]
    fn guardrail_format_check_results_failure_enforce() {
        let results = vec![CheckResult {
            command: "cargo clippy".into(),
            success: false,
            output: "warning: unused variable".into(),
        }];
        let msg = format_check_results(&results, GuardrailLevel::Enforce);
        assert!(msg.contains("GUARDRAIL CHECK FAILED"));
        assert!(msg.contains("enforce"));
        assert!(msg.contains("cargo clippy"));
    }

    #[test]
    fn guardrail_format_check_results_failure_advisory() {
        let results = vec![CheckResult {
            command: "mix test".into(),
            success: false,
            output: "1 test failed".into(),
        }];
        let msg = format_check_results(&results, GuardrailLevel::Advisory);
        assert!(msg.contains("advisory"));
        assert!(msg.contains("mix test"));
    }

    #[test]
    fn guardrail_merge_only_overrides_present_fields() {
        let mut base = GuardrailConfig {
            enabled: Some(true),
            level: Some(GuardrailLevel::Advisory),
            profile: Some(GuardrailProfile::Rust),
            critical_paths: Some(vec!["src/**".into()]),
            after_write: None,
        };

        let overlay = GuardrailConfig {
            enabled: None,
            level: Some(GuardrailLevel::Enforce),
            profile: None,
            critical_paths: None,
            after_write: Some(vec!["cargo test".into()]),
        };

        base.merge(overlay);

        assert_eq!(base.enabled, Some(true));
        assert_eq!(base.level, Some(GuardrailLevel::Enforce));
        assert_eq!(base.profile, Some(GuardrailProfile::Rust));
        assert_eq!(base.critical_paths, Some(vec!["src/**".into()]));
        assert_eq!(base.after_write, Some(vec!["cargo test".into()]));
    }
}
