use serde::{Deserialize, Serialize};

use crate::model::{AuditFinding, AuditReport, AuditSeverity, CheckStatus};

/// Compact aggregate counts for an audit report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSummary {
    /// Total checks included in the report.
    pub checks_total: usize,
    /// Checks that passed.
    pub checks_passed: usize,
    /// Checks that failed.
    pub checks_failed: usize,
    /// Checks skipped because they were optional or out of scope.
    pub checks_skipped: usize,
    /// Checks blocked before execution.
    pub checks_blocked: usize,
    /// Checks that timed out.
    pub checks_timed_out: usize,
    /// Total normalized findings.
    pub findings_total: usize,
    /// Findings marked as new relative to baseline.
    pub findings_new: usize,
    /// Error-severity findings.
    pub errors: usize,
    /// Warning-severity findings.
    pub warnings: usize,
    /// Short human-readable status line.
    pub compact: String,
}

/// Build a compact summary for model and UI display.
#[must_use]
pub fn summarize_report(report: &AuditReport) -> ReportSummary {
    let checks_total = report.checks.len();
    let checks_passed = count_checks(report, CheckStatus::Passed);
    let checks_failed = count_checks(report, CheckStatus::Failed);
    let checks_skipped = count_checks(report, CheckStatus::Skipped);
    let checks_blocked = count_checks(report, CheckStatus::Blocked);
    let checks_timed_out = count_checks(report, CheckStatus::TimedOut);
    let findings: Vec<&AuditFinding> = report.findings().collect();
    let findings_total = findings.len();
    let findings_new = findings.iter().filter(|finding| finding.is_new).count();
    let errors = findings
        .iter()
        .filter(|finding| finding.severity == AuditSeverity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|finding| finding.severity == AuditSeverity::Warning)
        .count();

    let compact = compact_status(
        report,
        checks_failed,
        checks_blocked,
        checks_timed_out,
        findings_total,
        findings_new,
    );

    ReportSummary {
        checks_total,
        checks_passed,
        checks_failed,
        checks_skipped,
        checks_blocked,
        checks_timed_out,
        findings_total,
        findings_new,
        errors,
        warnings,
        compact,
    }
}

fn count_checks(report: &AuditReport, status: CheckStatus) -> usize {
    report
        .checks
        .iter()
        .filter(|check| check.status == status)
        .count()
}

fn compact_status(
    report: &AuditReport,
    checks_failed: usize,
    checks_blocked: usize,
    checks_timed_out: usize,
    findings_total: usize,
    findings_new: usize,
) -> String {
    let status = if checks_blocked > 0 || checks_timed_out > 0 {
        "blocked"
    } else if checks_failed > 0 || findings_new > 0 {
        "failed"
    } else {
        "passed"
    };

    format!(
        "audit {status}: profile `{}`, scope `{}`, {findings_new} new findings, {findings_total} total findings",
        report.profile, report.scope
    )
}

#[cfg(test)]
mod tests {
    use crate::model::{AuditCheckRun, AuditFinding, AuditLocation};

    use super::*;

    #[test]
    fn summary_counts_checks_and_findings() {
        let report = AuditReport::new("quick", "changed").with_checks(vec![
            AuditCheckRun::new("fmt", "format", CheckStatus::Passed),
            AuditCheckRun::new("native", "ai_quality", CheckStatus::Failed).with_findings(vec![
                AuditFinding::new(
                    "audit/placeholder",
                    AuditSeverity::Error,
                    "placeholder implementation",
                    "native",
                )
                .with_location(AuditLocation::path("src/lib.rs"))
                .with_new(true),
                AuditFinding::new(
                    "audit/comment",
                    AuditSeverity::Warning,
                    "hedging comment",
                    "native",
                ),
            ]),
            AuditCheckRun::new("aislop", "ai_quality", CheckStatus::Skipped),
        ]);

        let summary = summarize_report(&report);
        assert_eq!(summary.checks_total, 3);
        assert_eq!(summary.checks_passed, 1);
        assert_eq!(summary.checks_failed, 1);
        assert_eq!(summary.checks_skipped, 1);
        assert_eq!(summary.findings_total, 2);
        assert_eq!(summary.findings_new, 1);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.warnings, 1);
        assert!(summary.compact.contains("audit failed"));
    }
}
