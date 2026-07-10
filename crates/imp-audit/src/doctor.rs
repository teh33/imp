use serde::{Deserialize, Serialize};

use crate::config::AuditConfig;
use crate::error::Result;
use crate::requirements::{
    availability_for_profile, RequirementAvailability, RequirementProbe, RequirementStatus,
};

/// Readiness report for a selected audit profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Profile that was inspected.
    pub profile: String,
    /// Requirement availability rows for checks in the profile.
    #[serde(default)]
    pub requirements: Vec<RequirementAvailability>,
    /// Aggregate readiness summary.
    pub summary: DoctorSummary,
}

impl DoctorReport {
    /// Returns true when the selected profile can run without blocked requirements.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.summary.blocking_missing == 0
    }
}

/// Aggregate counts for audit readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorSummary {
    /// Total external requirements referenced by the selected profile.
    pub total: usize,
    /// Requirements whose binaries were found.
    pub available: usize,
    /// Missing requirements used only by optional checks.
    pub optional_missing: usize,
    /// Missing requirements required by blocking checks.
    pub blocking_missing: usize,
    /// Compact human-readable status line.
    pub compact: String,
}

/// Build a doctor report for a selected profile without running checks.
pub fn doctor_report_for_profile(
    config: &AuditConfig,
    profile: &str,
    probe: &impl RequirementProbe,
) -> Result<DoctorReport> {
    let requirements = availability_for_profile(config, profile, probe)?;
    let summary = summarize_requirements(profile, &requirements);
    Ok(DoctorReport {
        profile: profile.to_string(),
        requirements,
        summary,
    })
}

/// Render a compact human-readable readiness table.
#[must_use]
pub fn format_doctor_report(report: &DoctorReport) -> String {
    let mut lines = vec![report.summary.compact.clone()];
    if report.requirements.is_empty() {
        lines.push("No external tool requirements for this profile.".into());
        return lines.join("\n");
    }

    lines.push("".into());
    lines.push("Audit tools".into());
    for requirement in &report.requirements {
        let marker = match requirement.status {
            RequirementStatus::Available => "✓",
            RequirementStatus::Missing if requirement.is_blocking() => "✗",
            RequirementStatus::Missing => "○",
        };
        let status = match requirement.status {
            RequirementStatus::Available => "available",
            RequirementStatus::Missing if requirement.is_blocking() => "missing required",
            RequirementStatus::Missing => "missing optional",
        };
        lines.push(format!(
            "{marker} {:<16} {:<16} {}",
            requirement.binary,
            status,
            requirement_usage(requirement)
        ));

        if requirement.status == RequirementStatus::Missing && !requirement.installers.is_empty() {
            for installer in &requirement.installers {
                lines.push(format!(
                    "  install: {} ({})",
                    installer.command.join(" "),
                    installer.label
                ));
            }
        }
    }

    lines.join("\n")
}

fn summarize_requirements(
    profile: &str,
    requirements: &[RequirementAvailability],
) -> DoctorSummary {
    let total = requirements.len();
    let available = requirements
        .iter()
        .filter(|requirement| requirement.status == RequirementStatus::Available)
        .count();
    let blocking_missing = requirements
        .iter()
        .filter(|requirement| requirement.is_blocking())
        .count();
    let optional_missing = requirements
        .iter()
        .filter(|requirement| {
            requirement.status == RequirementStatus::Missing && !requirement.is_blocking()
        })
        .count();

    let compact = if blocking_missing > 0 {
        format!(
            "audit doctor: profile `{profile}` blocked by {blocking_missing} missing required tools"
        )
    } else if optional_missing > 0 {
        format!(
            "audit doctor: profile `{profile}` ready with {optional_missing} optional tools missing"
        )
    } else {
        format!("audit doctor: profile `{profile}` ready")
    };

    DoctorSummary {
        total,
        available,
        optional_missing,
        blocking_missing,
        compact,
    }
}

fn requirement_usage(requirement: &RequirementAvailability) -> String {
    let mut parts = Vec::new();
    if !requirement.required_by.is_empty() {
        parts.push(format!(
            "required by {}",
            requirement.required_by.join(", ")
        ));
    }
    if !requirement.optional_for.is_empty() {
        parts.push(format!(
            "optional for {}",
            requirement.optional_for.join(", ")
        ));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::requirements::RequirementProbe;

    use super::*;

    struct StaticProbe {
        available: HashSet<String>,
    }

    impl StaticProbe {
        fn with_available(binaries: &[&str]) -> Self {
            Self {
                available: binaries.iter().map(|binary| binary.to_string()).collect(),
            }
        }
    }

    impl RequirementProbe for StaticProbe {
        fn is_available(&self, binary: &str) -> bool {
            self.available.contains(binary)
        }
    }

    #[test]
    fn doctor_reports_blocking_and_optional_missing_tools() {
        let config = AuditConfig::from_toml_str(
            r#"
            [profiles.quick]
            checks = ["optional-ai", "required-security"]

            [[checks]]
            id = "optional-ai"
            kind = "ai_quality"
            optional = true
            requires = ["aislop"]

            [[checks]]
            id = "required-security"
            kind = "security"
            requires = ["semgrep"]

            [[requirements]]
            id = "aislop"
            binary = "aislop"

            [[requirements.installers]]
            id = "npm-dev"
            label = "Install aislop"
            manager = "npm"
            command = ["npm", "install", "--save-dev", "aislop"]
            scope = "project"

            [[requirements]]
            id = "semgrep"
            binary = "semgrep"
            "#,
        )
        .expect("config should parse");

        let report = doctor_report_for_profile(&config, "quick", &StaticProbe::with_available(&[]))
            .expect("doctor report should build");

        assert!(!report.is_ready());
        assert_eq!(report.summary.total, 2);
        assert_eq!(report.summary.optional_missing, 1);
        assert_eq!(report.summary.blocking_missing, 1);

        let text = format_doctor_report(&report);
        assert!(text.contains("blocked by 1 missing required tools"));
        assert!(text.contains("○ aislop"));
        assert!(text.contains("✗ semgrep"));
        assert!(text.contains("npm install --save-dev aislop"));
    }

    #[test]
    fn doctor_reports_ready_when_no_requirements_are_missing() {
        let config = AuditConfig::from_toml_str(
            r#"
            [profiles.quick]
            checks = ["fmt"]

            [[checks]]
            id = "fmt"
            kind = "format"
            requires = ["cargo"]

            [[requirements]]
            id = "cargo"
            binary = "cargo"
            "#,
        )
        .expect("config should parse");

        let report =
            doctor_report_for_profile(&config, "quick", &StaticProbe::with_available(&["cargo"]))
                .expect("doctor report should build");

        assert!(report.is_ready());
        assert_eq!(report.summary.available, 1);
        assert_eq!(
            report.summary.compact,
            "audit doctor: profile `quick` ready"
        );
    }
}
