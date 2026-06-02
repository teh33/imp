use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use imp_core::trust::{Provenance, RiskLabel};

use super::{DisplayToolCall, TrustLabel};

#[derive(Debug, Clone)]
pub(super) struct TuiTrace {
    pub(super) path: PathBuf,
}

impl TuiTrace {
    pub(super) fn from_env() -> Option<Self> {
        Self::from_env_value(std::env::var_os("IMP_TUI_TRACE"))
    }

    pub(super) fn from_env_value(value: Option<std::ffi::OsString>) -> Option<Self> {
        value
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| Self { path })
    }

    pub(super) fn log(&self, message: impl AsRef<str>) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{} {}", imp_llm::now(), message.as_ref());
        }
    }
}

pub(super) fn trust_policy_warning(
    record: &imp_core::reference_monitor::PolicyTraceRecord,
) -> Option<String> {
    let reason = match &record.decision {
        imp_core::reference_monitor::ToolPolicyDecision::Allow { reasons } => reasons
            .iter()
            .find(|reason| reason.source == imp_core::reference_monitor::PolicySource::TrustLabel),
        imp_core::reference_monitor::ToolPolicyDecision::Deny { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::AskUser { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::DryRunOnly { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::SandboxOnly { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::RequireVerification { reason } => {
            (reason.source == imp_core::reference_monitor::PolicySource::TrustLabel)
                .then_some(reason)
        }
    }?;

    Some(format!(
        "Trust warning: {} ({})",
        reason.message, reason.code
    ))
}

pub(super) fn extension_policy_warning(
    record: &imp_core::reference_monitor::PolicyTraceRecord,
) -> Option<String> {
    fn is_extension_policy_source(source: imp_core::reference_monitor::PolicySource) -> bool {
        matches!(
            source,
            imp_core::reference_monitor::PolicySource::ToolManifest
                | imp_core::reference_monitor::PolicySource::ConfigPolicy
        )
    }

    let reason = match &record.decision {
        imp_core::reference_monitor::ToolPolicyDecision::Allow { reasons } => reasons
            .iter()
            .find(|reason| is_extension_policy_source(reason.source)),
        imp_core::reference_monitor::ToolPolicyDecision::Deny { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::AskUser { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::DryRunOnly { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::SandboxOnly { reason }
        | imp_core::reference_monitor::ToolPolicyDecision::RequireVerification { reason } => {
            is_extension_policy_source(reason.source).then_some(reason)
        }
    }?;

    Some(format!(
        "Extension policy: {} ({})",
        reason.message, reason.code
    ))
}

pub(super) fn provenance_warning(provenance: &Provenance) -> Option<String> {
    if provenance.trust == TrustLabel::ExternalUntrusted
        || provenance
            .risk
            .contains(&RiskLabel::PossiblePromptInjection)
        || provenance.risk.contains(&RiskLabel::ContainsInstructions)
    {
        Some(format!(
            "Trust warning: low-trust content observed from {} cannot authorize policy/tool escalation.",
            provenance.origin.as_deref().unwrap_or("unknown source")
        ))
    } else {
        None
    }
}

pub(super) fn trace_tui_to(trace: Option<&TuiTrace>, message: impl AsRef<str>) {
    if let Some(trace) = trace {
        trace.log(message);
    }
}

pub(super) fn selected_read_file_path_from_tool(
    tc: Option<&DisplayToolCall>,
    cwd: &Path,
) -> Option<PathBuf> {
    let tc = tc?;
    if tc.name != "read" {
        return None;
    }

    let path = tc.details.get("path")?.as_str()?.trim();
    if path.is_empty() {
        return None;
    }

    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

pub(super) fn open_path_in_editor(path: &Path) -> std::io::Result<()> {
    let editor = std::env::var_os("VISUAL").or_else(|| std::env::var_os("EDITOR"));
    if let Some(editor) = editor.filter(|value| !value.is_empty()) {
        return std::process::Command::new(editor)
            .arg(path)
            .spawn()
            .map(|_| ());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}
