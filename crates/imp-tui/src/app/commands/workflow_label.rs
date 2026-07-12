use super::*;

pub(in crate::app) fn workflow_prompt_bar_label_for_run(run: &WorkflowRunSummary) -> String {
    format!(
        "workflow: {} · {}",
        capped_workflow_label(&run.scope, 32),
        workflow_progress_bar(run.total_closed, run.total_units)
    )
}

fn capped_workflow_label(label: &str, max_chars: usize) -> String {
    let trimmed = label.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().take(max_chars).collect()
}

fn workflow_progress_bar(done: u32, total: u32) -> String {
    const WIDTH: usize = 10;
    const PARTIALS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    if total == 0 {
        return "░".repeat(WIDTH);
    }
    let ratio = (done as f64 / total as f64).clamp(0.0, 1.0);
    let scaled = ratio * WIDTH as f64;
    let full = scaled.floor() as usize;
    let remainder = scaled - full as f64;

    let mut bar = String::new();
    for _ in 0..full.min(WIDTH) {
        bar.push('█');
    }
    if full < WIDTH {
        if remainder > 0.0 {
            let partial_index = ((remainder * PARTIALS.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(PARTIALS.len() - 1);
            bar.push(PARTIALS[partial_index]);
        }
        while bar.chars().count() < WIDTH {
            bar.push('░');
        }
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_summary(scope: &str, closed: u32, total: u32, status: &str) -> WorkflowRunSummary {
        WorkflowRunSummary {
            run_id: "run-1".to_string(),
            scope: scope.to_string(),
            status: status.to_string(),
            total_units: total,
            total_closed: closed,
            total_failed: 0,
            total_awaiting_verify: 0,
            latest: None,
            logs: Vec::new(),
            agents: Vec::new(),
        }
    }

    #[test]
    fn prompt_bar_workflow_label_uses_capped_scope_and_progress_bar() {
        let label = workflow_prompt_bar_label_for_run(&run_summary(
            "Dynamic workflow patterns with a very long title",
            3,
            4,
            "running",
        ));

        assert_eq!(
            label,
            "workflow: Dynamic workflow patterns with a · ███████▌░░"
        );
    }

    #[test]
    fn workflow_progress_lifecycle_renders_empty_and_done_bars() {
        assert_eq!(workflow_progress_bar(0, 0), "░░░░░░░░░░");
        assert_eq!(workflow_progress_bar(10, 10), "██████████");
    }

    #[test]
    fn workflow_live_state_label_uses_current_closed_and_total_units() {
        let label = workflow_prompt_bar_label_for_run(&run_summary("Claims", 37, 50, "running"));

        assert_eq!(label, "workflow: Claims · ███████▍░░");
    }
}
