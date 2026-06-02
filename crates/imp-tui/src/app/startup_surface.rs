use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::theme::Theme;
use crate::views::sidebar::SidebarDetailRenderData;
use crate::views::startup::{
    action_block_height, summarize_lines, truncate_preview, visible_section_count,
    StartupPanelData, StartupSection,
};

use super::{RepoStatsState, StartupSkillHit, StartupWorkflowHit, StartupWorkflowItem};

pub(super) enum RepoStatsScanRoot {
    Scan(PathBuf),
    Skip(RepoStatsState),
}

pub(super) fn repo_stats_scan_root(cwd: &Path) -> RepoStatsScanRoot {
    if is_home_directory(cwd) {
        return RepoStatsScanRoot::Skip(RepoStatsState::HomeDirectory);
    }

    match find_git_root(cwd) {
        Some(root) if is_home_directory(&root) => {
            RepoStatsScanRoot::Skip(RepoStatsState::HomeDirectory)
        }
        Some(root) => RepoStatsScanRoot::Scan(root),
        None => RepoStatsScanRoot::Skip(RepoStatsState::NoRepo),
    }
}

fn is_home_directory(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    canonicalize_for_compare(path) == canonicalize_for_compare(&home)
}

fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(path) = dir {
        if path.join(".git").exists() {
            return Some(path.to_path_buf());
        }
        dir = path.parent();
    }
    None
}

fn canonicalize_for_compare(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn strip_status_suffix(summary: &str) -> String {
    summary
        .strip_suffix(" (ready)")
        .or_else(|| summary.strip_suffix(" (needs key)"))
        .unwrap_or(summary)
        .to_string()
}

pub(super) fn repo_stats_label(state: Option<&RepoStatsState>) -> String {
    match state {
        Some(RepoStatsState::Scanning) | None => "scanning…".to_string(),
        Some(RepoStatsState::Ready(stats)) => {
            let index_counts = match (stats.symbols, stats.tests) {
                (Some(symbols), Some(tests)) if tests > 0 => {
                    format!(
                        " · {} symbols · {} tests",
                        format_compact_count(symbols as u64),
                        format_compact_count(tests as u64)
                    )
                }
                (Some(symbols), _) => {
                    format!(" · {} symbols", format_compact_count(symbols as u64))
                }
                _ => String::new(),
            };
            format!(
                "{} · {} loc · {} files{}",
                stats.primary_language,
                format_compact_count(stats.code_lines),
                format_compact_count(stats.files),
                index_counts
            )
        }
        Some(RepoStatsState::HomeDirectory) => "home directory".to_string(),
        Some(RepoStatsState::NoRepo) => "none".to_string(),
        Some(RepoStatsState::Empty) => "no source files".to_string(),
        Some(RepoStatsState::Failed) => "unavailable".to_string(),
    }
}

fn format_compact_count(count: u64) -> String {
    if count >= 1_000_000 {
        trim_trailing_decimal(format!("{:.1}", count as f64 / 1_000_000.0)) + "m"
    } else if count >= 1_000 {
        trim_trailing_decimal(format!("{:.1}", count as f64 / 1_000.0)) + "k"
    } else {
        count.to_string()
    }
}

fn trim_trailing_decimal(value: String) -> String {
    value.strip_suffix(".0").unwrap_or(&value).to_string()
}

pub(super) fn discover_rule_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let global = PathBuf::from(home).join(".imp/AGENTS.md");
        if global.exists() {
            files.push(global);
        }
    }
    for ancestor in cwd.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let local = ancestor.join("AGENTS.md");
        if local.exists() {
            files.push(local);
        }
    }
    files
}

pub(super) fn rule_file_lines(files: &[PathBuf]) -> Vec<String> {
    if files.is_empty() {
        return vec!["• rules: none".to_string()];
    }
    let mut lines = Vec::new();
    for (index, path) in files.iter().take(3).enumerate() {
        let prefix = if index == 0 {
            "• rules: "
        } else {
            "         "
        };
        lines.push(format!("{prefix}{}", display_rule_path(path)));
    }
    if files.len() > 3 {
        lines.push(format!("         … +{} more", files.len() - 3));
    }
    lines
}

fn display_rule_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = home.as_deref() {
        if let Ok(rest) = path.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

pub(super) fn discover_startup_workflows(cwd: &Path) -> Vec<StartupWorkflowItem> {
    let workflows_root = cwd.join(".imp").join("workflows");
    let Ok(entries) = std::fs::read_dir(workflows_root) else {
        return Vec::new();
    };

    let mut workflows = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path().join("workflow.yaml");
            path.exists().then_some(path)
        })
        .filter_map(|path| load_startup_workflow_item(&path))
        .collect::<Vec<_>>();
    workflows.sort_by(|a, b| {
        workflow_status_rank(&a.status)
            .cmp(&workflow_status_rank(&b.status))
            .then(a.id.cmp(&b.id))
    });
    workflows
}

#[derive(Debug, serde::Deserialize)]
struct StartupWorkflowYaml {
    id: String,
    title: String,
    status: String,
    kind: String,
}

fn load_startup_workflow_item(path: &Path) -> Option<StartupWorkflowItem> {
    let content = std::fs::read_to_string(path).ok()?;
    let workflow = serde_yaml::from_str::<StartupWorkflowYaml>(&content).ok()?;
    Some(StartupWorkflowItem {
        id: workflow.id,
        title: workflow.title,
        status: workflow.status,
        kind: workflow.kind,
        path: path.to_path_buf(),
    })
}

fn workflow_status_rank(status: &str) -> u8 {
    match status {
        "active" => 0,
        "waiting" | "needs_context" => 1,
        "blocked" | "failed" => 2,
        "planned" => 3,
        "done_with_concerns" => 4,
        "done" | "cancelled" => 5,
        _ => 6,
    }
}

pub(super) fn workflow_startup_lines(workflows: &[StartupWorkflowItem]) -> Vec<String> {
    if workflows.is_empty() {
        return vec![
            "• none found".to_string(),
            "• create: ask imp to make a workflow plan".to_string(),
        ];
    }

    summarize_lines(
        workflows
            .iter()
            .map(|workflow| {
                format!(
                    "• {}: {} · {} · {}",
                    workflow.id, workflow.status, workflow.kind, workflow.title
                )
            })
            .collect(),
        6,
    )
}

pub(super) fn startup_workflow_detail_render_data(
    workflow: &StartupWorkflowItem,
    theme: &Theme,
) -> SidebarDetailRenderData {
    let mut plain_lines = vec![
        format!("workflow: {}", workflow.id),
        format!("title: {}", workflow.title),
        format!("status: {}", workflow.status),
        format!("kind: {}", workflow.kind),
        format!("path: {}", workflow.path.display()),
        String::new(),
    ];

    match std::fs::read_to_string(&workflow.path) {
        Ok(content) => plain_lines.extend(
            truncate_preview(&content, 24, 4000)
                .lines()
                .map(str::to_string),
        ),
        Err(err) => plain_lines.push(format!("Failed to read workflow: {err}")),
    }

    let lines = plain_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                Line::from(Span::styled(
                    line.clone(),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ))
            } else if index <= 4 && !line.is_empty() {
                Line::from(Span::styled(line.clone(), theme.muted_style()))
            } else {
                Line::from(Span::raw(line.clone()))
            }
        })
        .collect();

    SidebarDetailRenderData { lines, plain_lines }
}

pub(super) fn startup_skill_detail_render_data(
    skill: &imp_core::resources::Skill,
    theme: &Theme,
) -> SidebarDetailRenderData {
    let mut plain_lines = vec![
        format!("skill: {}", skill.name),
        format!("path: {}", skill.path.display()),
    ];
    if !skill.description.trim().is_empty() {
        plain_lines.push(format!("description: {}", skill.description.trim()));
    }
    plain_lines.push(String::new());

    match std::fs::read_to_string(&skill.path) {
        Ok(content) => plain_lines.extend(content.lines().map(str::to_string)),
        Err(err) => plain_lines.push(format!("Failed to read skill: {err}")),
    }

    let lines = plain_lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                Line::from(Span::styled(
                    line.clone(),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ))
            } else if index <= 2 && !line.is_empty() {
                Line::from(Span::styled(line.clone(), theme.muted_style()))
            } else {
                Line::from(Span::raw(line.clone()))
            }
        })
        .collect();

    SidebarDetailRenderData { lines, plain_lines }
}

pub(super) fn startup_skill_hits(area: Rect, panel: &StartupPanelData) -> Vec<StartupSkillHit> {
    startup_section_hits(
        area,
        panel,
        |section| section.title.starts_with("skills"),
        |index, rect| StartupSkillHit { index, rect },
    )
}

pub(super) fn startup_workflow_hits(
    area: Rect,
    panel: &StartupPanelData,
) -> Vec<StartupWorkflowHit> {
    startup_section_hits(
        area,
        panel,
        |section| section.title.starts_with("workflows"),
        |index, rect| StartupWorkflowHit { index, rect },
    )
}

fn startup_section_hits<T>(
    area: Rect,
    panel: &StartupPanelData,
    matches_section: impl Fn(&StartupSection) -> bool + Copy,
    make_hit: impl Fn(usize, Rect) -> T + Copy,
) -> Vec<T> {
    if area.width < 24 || area.height < 8 {
        return Vec::new();
    }

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let sections_area = if inner.height < 12 {
        let action_height = 3.min(inner.height);
        Rect {
            y: inner.y + action_height,
            height: inner.height.saturating_sub(action_height),
            ..inner
        }
    } else {
        let action_height = action_block_height(inner.width, panel.actions.len());
        Rect {
            y: inner.y + action_height,
            height: inner.height.saturating_sub(action_height),
            ..inner
        }
    };

    startup_hits_in_sections(sections_area, &panel.sections, matches_section, make_hit)
}

fn startup_hits_in_sections<T>(
    area: Rect,
    sections: &[StartupSection],
    matches_section: impl Fn(&StartupSection) -> bool + Copy,
    make_hit: impl Fn(usize, Rect) -> T + Copy,
) -> Vec<T> {
    if sections.is_empty() || area.height == 0 || area.width == 0 {
        return Vec::new();
    }

    let visible_count = visible_section_count(area.width, area.height, sections.len());
    let visible_sections = &sections[..visible_count];

    if area.width >= 96 {
        let column_width = area.width / visible_sections.len() as u16;
        let remainder = area.width % visible_sections.len() as u16;
        return visible_sections
            .iter()
            .enumerate()
            .flat_map(|(index, section)| {
                let x_offset = column_width * index as u16 + remainder.min(index as u16);
                let width = column_width + u16::from((index as u16) < remainder);
                let rect = Rect {
                    x: area.x + x_offset,
                    width,
                    ..area
                };
                startup_hits_in_section(rect, section, matches_section, make_hit)
            })
            .collect();
    }

    match visible_sections.len() {
        0 => Vec::new(),
        1 => startup_hits_in_section(area, &visible_sections[0], matches_section, make_hit),
        2 => {
            let rects = if area.width >= 90 {
                split_horizontal(area, &[50, 50])
            } else {
                split_vertical(area, &[50, 50])
            };
            visible_sections
                .iter()
                .zip(rects)
                .flat_map(|(section, rect)| {
                    startup_hits_in_section(rect, section, matches_section, make_hit)
                })
                .collect()
        }
        3 => {
            let rects = if area.width >= 120 {
                split_horizontal(area, &[33, 34, 33])
            } else if area.width >= 78 && area.height >= 12 {
                let rows = split_vertical(area, &[50, 50]);
                let top = split_horizontal(rows[0], &[50, 50]);
                vec![top[0], top[1], rows[1]]
            } else {
                split_vertical(area, &[34, 33, 33])
            };
            visible_sections
                .iter()
                .zip(rects)
                .flat_map(|(section, rect)| {
                    startup_hits_in_section(rect, section, matches_section, make_hit)
                })
                .collect()
        }
        _ => {
            let row_height = (area.height / visible_sections.len() as u16).max(3);
            visible_sections
                .iter()
                .enumerate()
                .flat_map(|(index, section)| {
                    let rect = Rect {
                        y: area.y + row_height * index as u16,
                        height: row_height,
                        ..area
                    };
                    startup_hits_in_section(rect, section, matches_section, make_hit)
                })
                .collect()
        }
    }
}

fn startup_hits_in_section<T>(
    area: Rect,
    section: &StartupSection,
    matches_section: impl Fn(&StartupSection) -> bool,
    make_hit: impl Fn(usize, Rect) -> T,
) -> Vec<T> {
    if !matches_section(section) || area.height < 3 || area.width < 12 {
        return Vec::new();
    }

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    section
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.strip_prefix("• ").is_some_and(|name| {
                name != "none discovered" && name != "none found" && !name.starts_with("create:")
            })
        })
        .filter_map(|(index, _)| {
            let y = inner.y + index as u16;
            (y < inner.y + inner.height).then_some(make_hit(
                index,
                Rect {
                    y,
                    height: 1,
                    ..inner
                },
            ))
        })
        .collect()
}

fn split_horizontal(area: Rect, percentages: &[u16]) -> Vec<Rect> {
    let mut x = area.x;
    let mut used = 0u16;
    percentages
        .iter()
        .enumerate()
        .map(|(index, pct)| {
            let width = if index + 1 == percentages.len() {
                area.width.saturating_sub(used)
            } else {
                area.width * *pct / 100
            };
            let rect = Rect { x, width, ..area };
            x = x.saturating_add(width);
            used = used.saturating_add(width);
            rect
        })
        .collect()
}

fn split_vertical(area: Rect, percentages: &[u16]) -> Vec<Rect> {
    let mut y = area.y;
    let mut used = 0u16;
    percentages
        .iter()
        .enumerate()
        .map(|(index, pct)| {
            let height = if index + 1 == percentages.len() {
                area.height.saturating_sub(used)
            } else {
                area.height * *pct / 100
            };
            let rect = Rect { y, height, ..area };
            y = y.saturating_add(height);
            used = used.saturating_add(height);
            rect
        })
        .collect()
}
