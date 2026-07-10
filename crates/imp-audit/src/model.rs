use serde::{Deserialize, Serialize};

/// Severity assigned to an audit finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AuditSeverity {
    /// A finding that should block completion when it is new and in scope.
    Error,
    /// A finding that should be reviewed but may not block completion.
    Warning,
    /// Informational finding.
    Info,
    /// Low-priority hint.
    Hint,
    /// Severity could not be inferred from the source tool.
    Unknown,
}

impl Default for AuditSeverity {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Status of an executed audit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckStatus {
    /// The check completed successfully and produced no blocking findings.
    Passed,
    /// The check completed and found issues or returned a failing exit status.
    Failed,
    /// The check was intentionally skipped, usually because an optional tool is missing.
    Skipped,
    /// The check could not run and should block the requested profile.
    Blocked,
    /// The check exceeded its timeout.
    TimedOut,
}

/// Source location for an audit finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLocation {
    /// Repository-relative or absolute path reported by the checker.
    pub path: String,
    /// One-based line number, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// One-based column number, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl AuditLocation {
    /// Create a path-only audit location.
    #[must_use]
    pub fn path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: None,
            column: None,
        }
    }
}

/// Normalized finding emitted by a native rule or external tool adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    /// Stable rule identifier, such as `audit/placeholder` or `clippy::unwrap_used`.
    pub rule_id: String,
    /// Finding severity.
    pub severity: AuditSeverity,
    /// Human-readable finding message.
    pub message: String,
    /// Tool or engine that produced the finding.
    pub source: String,
    /// Primary location, when the finding maps to source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<AuditLocation>,
    /// True when baseline comparison classifies this finding as newly introduced.
    #[serde(default)]
    pub is_new: bool,
}

impl AuditFinding {
    /// Construct a finding with the required fields.
    #[must_use]
    pub fn new(
        rule_id: impl Into<String>,
        severity: AuditSeverity,
        message: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            severity,
            message: message.into(),
            source: source.into(),
            location: None,
            is_new: false,
        }
    }

    /// Attach a source location to the finding.
    #[must_use]
    pub fn with_location(mut self, location: AuditLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Mark whether this finding is new relative to baseline.
    #[must_use]
    pub fn with_new(mut self, is_new: bool) -> Self {
        self.is_new = is_new;
        self
    }
}

/// Execution outcome for one audit check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditCheckRun {
    /// Check identifier.
    pub id: String,
    /// Check kind, such as `lint`, `test`, `security`, or `ai_quality`.
    pub kind: String,
    /// Check execution status.
    pub status: CheckStatus,
    /// Optional short summary for display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Normalized findings from this check.
    #[serde(default)]
    pub findings: Vec<AuditFinding>,
}

impl AuditCheckRun {
    /// Construct a check run with no findings.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: impl Into<String>, status: CheckStatus) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            status,
            summary: None,
            findings: Vec::new(),
        }
    }

    /// Attach a short summary.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Attach findings.
    #[must_use]
    pub fn with_findings(mut self, findings: Vec<AuditFinding>) -> Self {
        self.findings = findings;
        self
    }
}

/// Full normalized audit report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    /// Profile that was run, such as `quick` or `security`.
    pub profile: String,
    /// Scope that was audited, such as `changed` or `workspace`.
    pub scope: String,
    /// Check results included in this report.
    #[serde(default)]
    pub checks: Vec<AuditCheckRun>,
}

impl AuditReport {
    /// Create an empty report.
    #[must_use]
    pub fn new(profile: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
            scope: scope.into(),
            checks: Vec::new(),
        }
    }

    /// Attach check runs.
    #[must_use]
    pub fn with_checks(mut self, checks: Vec<AuditCheckRun>) -> Self {
        self.checks = checks;
        self
    }

    /// Iterate over all findings in the report.
    pub fn findings(&self) -> impl Iterator<Item = &AuditFinding> {
        self.checks.iter().flat_map(|check| check.findings.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_round_trips_through_json() {
        let report = AuditReport::new("quick", "changed").with_checks(vec![AuditCheckRun::new(
            "native-agent-quality",
            "ai_quality",
            CheckStatus::Failed,
        )
        .with_findings(vec![AuditFinding::new(
            "audit/placeholder",
            AuditSeverity::Error,
            "placeholder implementation",
            "native",
        )
        .with_location(AuditLocation::path("src/lib.rs"))
        .with_new(true)])]);

        let json = serde_json::to_string(&report).expect("report should serialize");
        let decoded: AuditReport = serde_json::from_str(&json).expect("report should deserialize");
        assert_eq!(decoded, report);
    }
}
