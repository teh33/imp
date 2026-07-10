use super::{Agent, ContinueReason};

impl Agent {
    pub(crate) fn task_closeout_follow_up(&self) -> Option<String> {
        let issues = self
            .task_state
            .lock()
            .expect("session task state lock")
            .closeout_issues();
        (!issues.is_empty()).then(|| {
            format!(
                "Task closeout is incomplete: {}. Continue working, resolve the runtime evidence, and update task steps before finishing.",
                issues.join("; ")
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectiveKind {
    Explain,
    Research,
    Plan,
    Implement,
    Orchestrate,
    Review,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutonomousObjective {
    pub(crate) prompt: String,
    pub(crate) kind: ObjectiveKind,
}

impl AutonomousObjective {
    pub(crate) fn from_prompt(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            kind: classify_objective_kind(prompt),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObligationKind {
    FailedCommandRecovery,
    EditedFilesVerification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Obligation {
    pub(crate) kind: ObligationKind,
    pub(crate) prompt: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ObligationLedger {
    obligations: Vec<Obligation>,
}

impl ObligationLedger {
    pub(crate) fn add(&mut self, obligation: Obligation) {
        if self
            .obligations
            .iter()
            .any(|existing| existing.kind == obligation.kind)
        {
            return;
        }
        self.obligations.push(obligation);
    }

    pub(crate) fn resolve_kind(&mut self, kind: ObligationKind) {
        self.obligations
            .retain(|obligation| obligation.kind != kind);
    }

    pub(crate) fn next_continue(&self) -> Option<(String, ContinueReason)> {
        self.obligations.first().map(|obligation| {
            let reason = match obligation.kind {
                ObligationKind::FailedCommandRecovery | ObligationKind::EditedFilesVerification => {
                    ContinueReason::ExecutionDebt
                }
            };
            (obligation.prompt.clone(), reason)
        })
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, kind: ObligationKind) -> bool {
        self.obligations
            .iter()
            .any(|obligation| obligation.kind == kind)
    }
}

pub(crate) fn failed_command_recovery_obligation() -> Obligation {
    Obligation {
        kind: ObligationKind::FailedCommandRecovery,
        prompt: super::failed_bash_recovery_follow_up_text().to_string(),
    }
}

pub(crate) fn edited_files_verification_obligation() -> Obligation {
    Obligation {
        kind: ObligationKind::EditedFilesVerification,
        prompt: "Files were changed. Review the diff, run the narrowest relevant verification command, and fix any failures before summarizing completion.".to_string(),
    }
}

fn classify_objective_kind(prompt: &str) -> ObjectiveKind {
    let lower = prompt.to_ascii_lowercase();
    if lower.contains("research") || lower.contains("investigate") || lower.contains("audit") {
        ObjectiveKind::Research
    } else if lower.contains("plan") || lower.contains("design") {
        ObjectiveKind::Plan
    } else if lower.contains("implement")
        || lower.contains("fix")
        || lower.contains("build")
        || lower.contains("wire")
    {
        ObjectiveKind::Implement
    } else if lower.contains("review") {
        ObjectiveKind::Review
    } else if lower.contains("orchestrate")
        || lower.contains("delegate")
        || lower.contains("parallel")
    {
        ObjectiveKind::Orchestrate
    } else if lower.starts_with("how ")
        || lower.starts_with("what ")
        || lower.starts_with("why ")
        || lower.contains("explain")
    {
        ObjectiveKind::Explain
    } else {
        ObjectiveKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objective_classifies_explanation_without_work_intent() {
        let objective = AutonomousObjective::from_prompt("How do workflows differ from skills?");
        assert_eq!(objective.kind, ObjectiveKind::Explain);
    }

    #[test]
    fn obligation_ledger_deduplicates_and_resolves_failed_command_recovery() {
        let mut ledger = ObligationLedger::default();
        ledger.add(failed_command_recovery_obligation());
        ledger.add(failed_command_recovery_obligation());

        assert!(ledger.contains(ObligationKind::FailedCommandRecovery));
        let next = ledger.next_continue().expect("obligation should continue");
        assert_eq!(next.1, ContinueReason::ExecutionDebt);
        assert!(next
            .0
            .contains("failed command is usually diagnostic evidence"));

        ledger.resolve_kind(ObligationKind::FailedCommandRecovery);
        assert!(!ledger.contains(ObligationKind::FailedCommandRecovery));
        assert!(ledger.next_continue().is_none());
    }
}
