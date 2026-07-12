mod codex;
mod opencode;
mod pi;

use std::fs;
use std::path::{Path, PathBuf};

use clap::ValueEnum;

use super::process::ProcessOutcome;
use super::result::EvalAgentResult;
use super::spec::EvalTaskSpec;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EvalAgentKind {
    Imp,
    Pi,
    Codex,
    Opencode,
}

impl EvalAgentKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Imp => "imp",
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }

    pub(super) fn invocation(
        self,
        spec: &EvalTaskSpec,
        provider: &str,
        model: &str,
        thinking: &str,
        cwd: &Path,
    ) -> Result<AgentInvocation, Box<dyn std::error::Error>> {
        let mut invocation = AgentInvocation::default();
        invocation.args = match self {
            Self::Imp => imp_args(spec, provider, model, thinking),
            Self::Pi => pi::args(spec, provider, model, thinking),
            Self::Codex => codex::args(spec, model, thinking),
            Self::Opencode => opencode::args(spec, provider, model, thinking, cwd),
        };
        if matches!(self, Self::Codex | Self::Opencode) {
            let config_dir = isolated_config_dir(self)?;
            invocation.env = match self {
                Self::Codex => codex::env(&config_dir)?,
                Self::Opencode => opencode::env(&config_dir)?,
                _ => Vec::new(),
            };
            invocation.config_dir = Some(config_dir);
        }
        Ok(invocation)
    }

    pub(super) fn record_outcome(
        self,
        result: &mut EvalAgentResult,
        process: ProcessOutcome,
        stdout_path: &Path,
    ) -> bool {
        result.exit_code = process.exit_code;
        result.timed_out = process.timed_out;
        result.duration_ms = Some(process.duration_ms);
        match self {
            Self::Imp => record_imp_outcome(result, process, stdout_path),
            Self::Pi => pi::record_outcome(result, process, stdout_path),
            Self::Codex => codex::record_outcome(result, process, stdout_path),
            Self::Opencode => opencode::record_outcome(result, process, stdout_path),
        }
    }
}

#[derive(Default)]
pub(super) struct AgentInvocation {
    pub(super) args: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    config_dir: Option<PathBuf>,
}

impl Drop for AgentInvocation {
    fn drop(&mut self) {
        if let Some(path) = self.config_dir.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

pub(crate) fn resolve_binary(
    kind: EvalAgentKind,
    override_path: Option<&Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = override_path {
        return path.canonicalize().map_err(|error| {
            format!(
                "failed to resolve {} eval binary {}: {error}",
                kind.name(),
                path.display()
            )
            .into()
        });
    }
    let env_name = format!("{}_EVAL_BINARY", kind.name().to_ascii_uppercase());
    if let Some(path) = std::env::var_os(env_name) {
        return Ok(PathBuf::from(path));
    }
    if kind == EvalAgentKind::Imp {
        let current = std::env::current_exe()?;
        if current
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == "imp" || value == "imp.exe")
        {
            return Ok(current);
        }
    }
    Ok(PathBuf::from(kind.name()))
}

fn isolated_config_dir(kind: EvalAgentKind) -> Result<PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!(
        "imp-eval-{}-{}",
        kind.name(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

fn imp_args(spec: &EvalTaskSpec, provider: &str, model: &str, thinking: &str) -> Vec<String> {
    vec![
        "--provider".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        "--thinking".into(),
        thinking.into(),
        "--no-session".into(),
        "--max-turns".into(),
        spec.max_turns.unwrap_or(50).to_string(),
        "--output".into(),
        "json".into(),
        "--print".into(),
        spec.execution_prompt(),
    ]
}

fn record_imp_outcome(
    result: &mut EvalAgentResult,
    process: ProcessOutcome,
    stdout_path: &Path,
) -> bool {
    result.outcome = fs::read_to_string(stdout_path)
        .ok()
        .and_then(|content| serde_json::from_str(content.trim()).ok());
    process.success()
        && result
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.get("status"))
            .and_then(|status| status.as_str())
            .is_some_and(|status| matches!(status, "done" | "done_with_concerns"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imp_outcome_requires_successful_structured_status() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.json");
        let process = ProcessOutcome {
            exit_code: Some(0),
            timed_out: false,
            duration_ms: 1,
        };

        fs::write(&output, r#"{"status":"blocked"}"#).unwrap();
        assert!(!record_imp_outcome(
            &mut EvalAgentResult::default(),
            process,
            &output
        ));

        fs::write(&output, r#"{"status":"done"}"#).unwrap();
        assert!(record_imp_outcome(
            &mut EvalAgentResult::default(),
            process,
            &output
        ));
    }

    #[test]
    fn explicit_binary_path_is_absolute() {
        let path = resolve_binary(EvalAgentKind::Imp, Some(Path::new("Cargo.toml"))).unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with("Cargo.toml"));
    }

    #[test]
    fn external_agents_have_isolated_invocations() {
        let spec = EvalTaskSpec {
            id: "task".into(),
            repo: String::new(),
            commit: String::new(),
            prompt: "fix".into(),
            verifier: "true".into(),
            fixture: Some("fixture".into()),
            expectations: Default::default(),
            setup: None,
            max_turns: None,
            timeout_seconds: None,
            verifier_timeout_seconds: None,
        };
        let invocation = EvalAgentKind::Opencode
            .invocation(
                &spec,
                "openai-codex",
                "gpt-5.6-sol",
                "xhigh",
                Path::new("."),
            )
            .unwrap();
        assert!(invocation.args.contains(&"--pure".to_string()));
        assert!(invocation
            .env
            .iter()
            .any(|(name, _)| name == "OPENCODE_DISABLE_EXTERNAL_SKILLS"));
    }
}
