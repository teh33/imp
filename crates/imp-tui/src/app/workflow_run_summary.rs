use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::theme::Theme;
use crate::views::sidebar::SidebarDetailRenderData;

use super::WorkflowRunSummary;

#[cfg(feature = "mana-ui")]
pub(super) fn workflow_run_summary_cache_key(run: &WorkflowRunSummary) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        run.run_id,
        run.scope,
        run.status,
        run.total_units,
        run.total_closed,
        run.total_failed,
        run.total_awaiting_verify,
        run.latest.as_deref().unwrap_or(""),
        run.logs.join("\n")
    )
}

#[cfg(not(feature = "mana-ui"))]
pub(super) fn workflow_run_detail_render_data(
    run: &WorkflowRunSummary,
    theme: &Theme,
) -> SidebarDetailRenderData {
    let lines = vec![Line::from(vec![
        Span::styled("╭─", theme.muted_style()),
        Span::styled(
            " workflow run ",
            theme.accent_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled("─╮", theme.muted_style()),
    ])];
    let plain_lines = vec![
        format!("run: {}", run.run_id),
        "Workflow run details are unavailable in this standalone build.".to_string(),
    ];
    SidebarDetailRenderData { lines, plain_lines }
}

#[cfg(feature = "mana-ui")]
pub(super) fn workflow_run_detail_render_data(
    run: &WorkflowRunSummary,
    theme: &Theme,
) -> SidebarDetailRenderData {
    let mut lines = vec![Line::from(vec![
        Span::styled("╭─", theme.muted_style()),
        Span::styled(
            " workflow run ",
            theme.accent_style().add_modifier(Modifier::BOLD),
        ),
        Span::styled("─╮", theme.muted_style()),
    ])];
    let mut plain_lines = vec![
        format!("run: {}", run.run_id),
        format!("status: {}", run.status),
        format!("scope: {}", run.scope),
        format!(
            "units: {} closed / {} total",
            run.total_closed, run.total_units
        ),
        format!("failed: {}", run.total_failed),
        format!("awaiting verify: {}", run.total_awaiting_verify),
    ];
    if !run.agents.is_empty() {
        plain_lines.push("agents:".to_string());
        for agent in run.agents.iter().take(8) {
            plain_lines.push(format!(
                "  {} {} · {} · {}",
                agent.unit_id, agent.status, agent.action, agent.title
            ));
        }
    }
    let recent_logs = run.logs.iter().rev().take(12).collect::<Vec<_>>();
    if recent_logs.is_empty() {
        plain_lines.push("log: —".to_string());
    } else {
        plain_lines.push("log:".to_string());
        for log in recent_logs.into_iter().rev() {
            plain_lines.push(format!("  {log}"));
        }
    }
    for (index, line) in plain_lines.iter().enumerate() {
        let style = if index == 0 || index == 1 {
            theme.accent_style()
        } else if line == "log: —" || line.ends_with('—') {
            theme.muted_style()
        } else if line.starts_with("failed:") && !line.ends_with('0') {
            theme.warning_style()
        } else {
            theme.style()
        };
        lines.push(Line::from(Span::styled(line.clone(), style)));
    }
    SidebarDetailRenderData { lines, plain_lines }
}
