use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvidencePacket {
    pub run_id: String,
    pub workflow_id: Option<String>,
    pub session_id: Option<String>,
    pub objective: String,
    pub workflow_type: Option<String>,
    pub risk_level: Option<String>,
    pub final_status: Option<String>,
    pub summary: Vec<String>,
    pub plan: Vec<String>,
    pub actions: EvidenceActions,
    pub policy: EvidencePolicy,
    pub trust: EvidenceTrustSummary,
    pub verification: Vec<EvidenceVerificationGate>,
    pub artifacts: Vec<EvidenceArtifact>,
    pub concerns: Vec<String>,
    pub next_steps: Vec<String>,
}

impl EvidencePacket {
    pub fn new(run_id: impl Into<String>, objective: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            objective: objective.into(),
            ..Self::default()
        }
    }

    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Evidence Packet\n\n");
        self.render_workflow(&mut out);
        self.render_summary(&mut out);
        self.render_plan(&mut out);
        self.render_actions(&mut out);
        self.render_policy(&mut out);
        self.render_trust(&mut out);
        self.render_verification(&mut out);
        self.render_artifacts(&mut out);
        self.render_closeout(&mut out);
        out
    }

    pub fn write_markdown(&self, path: impl AsRef<Path>) -> io::Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.render_markdown())
    }

    fn render_workflow(&self, out: &mut String) {
        out.push_str("## Workflow\n\n");
        bullet(out, "Run", &self.run_id);
        optional_bullet(out, "Workflow", self.workflow_id.as_deref());
        optional_bullet(out, "Session", self.session_id.as_deref());
        bullet(out, "Objective", &safe_inline(&self.objective));
        optional_bullet(out, "Type", self.workflow_type.as_deref());
        optional_bullet(out, "Risk", self.risk_level.as_deref());
        out.push('\n');
    }

    fn render_summary(&self, out: &mut String) {
        out.push_str("## Summary\n\n");
        optional_bullet(out, "Final status", self.final_status.as_deref());
        render_list_or_none(out, &self.summary);
        out.push('\n');
    }

    fn render_plan(&self, out: &mut String) {
        out.push_str("## Plan\n\n");
        render_list_or_none(out, &self.plan);
        out.push('\n');
    }

    fn render_actions(&self, out: &mut String) {
        out.push_str("## Actions\n\n");
        render_named_list(out, "Files inspected", &self.actions.files_inspected);
        render_named_list(out, "Files changed", &self.actions.files_changed);
        render_named_list(out, "Commands run", &self.actions.commands_run);
        render_named_list(out, "Searches", &self.actions.searches);
        render_named_list(out, "Tools", &self.actions.tools);
        out.push('\n');
    }

    fn render_policy(&self, out: &mut String) {
        out.push_str("## Policy\n\n");
        render_named_list(out, "Decisions", &self.policy.decisions);
        render_named_list(out, "Denials", &self.policy.denials);
        render_named_list(out, "Approvals", &self.policy.approvals);
        out.push('\n');
    }

    fn render_trust(&self, out: &mut String) {
        out.push_str("## Trust & Provenance\n\n");
        render_named_list(out, "Sources", &self.trust.sources);
        render_named_list(
            out,
            "Low-trust influences",
            &self.trust.low_trust_influences,
        );
        render_named_list(out, "Warnings", &self.trust.warnings);
        out.push('\n');
    }

    fn render_verification(&self, out: &mut String) {
        out.push_str("## Verification\n\n");
        if self.verification.is_empty() {
            out.push_str("No verification gates were declared.\n\n");
            return;
        }
        for gate in &self.verification {
            out.push_str(&format!(
                "- **{}**: {}",
                safe_inline(&gate.name),
                safe_inline(&gate.status)
            ));
            if gate.required {
                out.push_str(" (required)");
            } else {
                out.push_str(" (optional)");
            }
            if let Some(command) = &gate.command {
                out.push_str(&format!(" — `{}`", safe_inline(command)));
            }
            if let Some(exit_code) = gate.exit_code {
                out.push_str(&format!(" — exit {exit_code}"));
            }
            if let Some(artifact) = &gate.artifact_path {
                out.push_str(&format!(" — `{}`", artifact.display()));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    fn render_artifacts(&self, out: &mut String) {
        out.push_str("## Artifacts\n\n");
        if self.artifacts.is_empty() {
            out.push_str("None recorded.\n\n");
            return;
        }
        for artifact in &self.artifacts {
            out.push_str(&format!(
                "- **{}**: `{}`",
                safe_inline(&artifact.kind),
                artifact.path.display()
            ));
            if let Some(summary) = &artifact.summary {
                out.push_str(&format!(" — {}", safe_inline(summary)));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    fn render_closeout(&self, out: &mut String) {
        out.push_str("## Closeout\n\n");
        optional_bullet(out, "Final status", self.final_status.as_deref());
        render_named_list(out, "Concerns", &self.concerns);
        render_named_list(out, "Next steps", &self.next_steps);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvidenceActions {
    pub files_inspected: Vec<String>,
    pub files_changed: Vec<String>,
    pub commands_run: Vec<String>,
    pub searches: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvidencePolicy {
    pub decisions: Vec<String>,
    pub denials: Vec<String>,
    pub approvals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvidenceTrustSummary {
    pub sources: Vec<String>,
    pub low_trust_influences: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceVerificationGate {
    pub name: String,
    pub required: bool,
    pub status: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub artifact_path: Option<PathBuf>,
}

impl Default for EvidenceVerificationGate {
    fn default() -> Self {
        Self {
            name: String::new(),
            required: true,
            status: "pending".into(),
            command: None,
            exit_code: None,
            artifact_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceArtifact {
    pub kind: String,
    pub path: PathBuf,
    pub summary: Option<String>,
}

impl Default for EvidenceArtifact {
    fn default() -> Self {
        Self {
            kind: String::new(),
            path: PathBuf::new(),
            summary: None,
        }
    }
}

fn bullet(out: &mut String, label: &str, value: &str) {
    out.push_str(&format!("- **{label}:** {value}\n"));
}

fn optional_bullet(out: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        bullet(out, label, &safe_inline(value));
    }
}

fn render_named_list(out: &mut String, label: &str, values: &[String]) {
    out.push_str(&format!("### {label}\n\n"));
    render_list_or_none(out, values);
    out.push('\n');
}

fn render_list_or_none(out: &mut String, values: &[String]) {
    if values.is_empty() {
        out.push_str("None recorded.\n");
    } else {
        for value in values {
            out.push_str(&format!("- {}\n", safe_inline(value)));
        }
    }
}

fn safe_inline(value: &str) -> String {
    const MAX: usize = 4 * 1024;
    let single_line = value.replace('\n', " ");
    if single_line.chars().count() > MAX {
        format!(
            "{}…[truncated]",
            single_line.chars().take(MAX).collect::<String>()
        )
    } else {
        single_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_packet_renders_expected_sections() {
        let packet = EvidencePacket {
            run_id: "run_1".into(),
            workflow_id: Some("394.4".into()),
            objective: "Emit evidence".into(),
            final_status: Some("DONE".into()),
            summary: vec!["Implemented renderer".into()],
            plan: vec!["Create model".into()],
            actions: EvidenceActions {
                files_changed: vec!["crates/imp-core/src/evidence.rs".into()],
                commands_run: vec!["cargo test -p imp-core evidence_packet".into()],
                ..EvidenceActions::default()
            },
            policy: EvidencePolicy {
                decisions: vec!["allow-all mode was active".into()],
                ..EvidencePolicy::default()
            },
            verification: vec![EvidenceVerificationGate {
                name: "unit tests".into(),
                status: "passed".into(),
                command: Some("cargo test".into()),
                exit_code: Some(0),
                artifact_path: Some(".imp/runs/run_1/verify.log".into()),
                ..EvidenceVerificationGate::default()
            }],
            artifacts: vec![EvidenceArtifact {
                kind: "trace".into(),
                path: ".imp/runs/run_1/trace.jsonl".into(),
                summary: None,
            }],
            concerns: vec![],
            next_steps: vec!["Wire runtime collection".into()],
            ..EvidencePacket::default()
        };

        let markdown = packet.render_markdown();
        for heading in [
            "# Evidence Packet",
            "## Workflow",
            "## Summary",
            "## Plan",
            "## Actions",
            "## Policy",
            "## Verification",
            "## Artifacts",
            "## Closeout",
        ] {
            assert!(markdown.contains(heading), "missing {heading}");
        }
        assert!(markdown.contains("allow-all"));
        assert!(markdown.contains("unit tests"));
    }

    #[test]
    fn evidence_packet_writes_markdown_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("run").join("evidence.md");
        EvidencePacket::new("run_1", "Test write")
            .write_markdown(&path)
            .unwrap();
        let markdown = std::fs::read_to_string(path).unwrap();
        assert!(markdown.contains("# Evidence Packet"));
        assert!(markdown.contains("Test write"));
    }
}
