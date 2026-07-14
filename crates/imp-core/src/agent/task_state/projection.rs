use imp_llm::Message;

use super::{SessionTaskState, TaskStepStatus};

impl SessionTaskState {
    pub fn projection(&self) -> String {
        let mut lines = vec!["<session_task>".to_string()];
        lines.push(format!("Objective: {}", self.objective.trim()));
        if !self.steps.is_empty() {
            lines.push("Steps:".into());
            lines.extend(self.steps.iter().map(|step| {
                let note = step
                    .note
                    .as_deref()
                    .map(|note| format!(" ({note})"))
                    .unwrap_or_default();
                format!(
                    "- {} [{}] {}{}",
                    step.id,
                    status_name(step.status),
                    step.description,
                    note
                )
            }));
        }
        push_list(&mut lines, "Constraints", &self.constraints);
        push_list(
            &mut lines,
            "Changed paths",
            &self.changed_paths.iter().cloned().collect::<Vec<_>>(),
        );
        push_list(&mut lines, "Blockers", &self.blockers);
        let unresolved_failures = self.unresolved_failures();
        push_list(&mut lines, "Unresolved failures", &unresolved_failures);
        if self.verification_required {
            lines.push("Verification: required after file changes".into());
        } else if let Some(check) = self.checks.last() {
            lines.push(format!(
                "Latest command: {} ({})",
                check.command,
                if check.passed { "passed" } else { "failed" }
            ));
        }
        if self.planning_active() {
            lines.push(
                "Planning is active. Use `task` for durable plan, constraint, and blocker updates; runtime evidence remains authoritative."
                    .into(),
            );
        }
        lines.push(
            "Use runtime evidence as authoritative. Do not claim completion while blockers, failures, pending steps, or required verification remain."
                .into(),
        );
        lines.push("</session_task>".into());
        lines.join("\n")
    }

    pub fn project_messages(&self, messages: &[Message]) -> Vec<Message> {
        if !self.should_project() {
            return messages.to_vec();
        }
        let mut projected = messages.to_vec();
        let trailing_results = projected
            .iter()
            .rev()
            .take_while(|message| matches!(message, Message::ToolResult(_)))
            .count();
        let index = projected.len().saturating_sub(trailing_results);
        projected.insert(index, Message::user(self.projection()));
        projected
    }
}

pub(super) fn status_name(status: TaskStepStatus) -> &'static str {
    match status {
        TaskStepStatus::Pending => "pending",
        TaskStepStatus::InProgress => "in_progress",
        TaskStepStatus::Completed => "completed",
        TaskStepStatus::Blocked => "blocked",
    }
}

fn push_list(lines: &mut Vec<String>, label: &str, values: &[String]) {
    if !values.is_empty() {
        lines.push(format!("{label}: {}", values.join("; ")));
    }
}
