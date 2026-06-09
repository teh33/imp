use std::collections::{BTreeMap, BTreeSet};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlashCommandKind {
    Builtin,
    Extension,
    Workflow,
    Skill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPalettePage {
    Commands,
    Extensions,
    Skills,
    Workflows,
}

impl CommandPalettePage {
    const ALL: [Self; 4] = [
        Self::Commands,
        Self::Extensions,
        Self::Skills,
        Self::Workflows,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Commands => "Commands",
            Self::Extensions => "Extensions",
            Self::Skills => "Skills",
            Self::Workflows => "Workflows",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Extensions => "extension commands",
            Self::Skills => "skills",
            Self::Workflows => "workflows",
        }
    }

    fn includes(self, kind: SlashCommandKind) -> bool {
        match self {
            Self::Commands => kind == SlashCommandKind::Builtin,
            Self::Extensions => kind == SlashCommandKind::Extension,
            Self::Skills => kind == SlashCommandKind::Skill,
            Self::Workflows => kind == SlashCommandKind::Workflow,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap_or(0)
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        let index = self.index();
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl SlashCommandKind {
    fn label(self) -> &'static str {
        match self {
            Self::Builtin => "commands",
            Self::Extension => "extensions",
            Self::Workflow => "workflows",
            Self::Skill => "skills",
        }
    }
}

/// A slash command definition.
#[derive(Debug, Clone)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub kind: SlashCommandKind,
}

impl SlashCommand {
    pub fn invocation(&self) -> String {
        match self.kind {
            SlashCommandKind::Builtin => self.name.clone(),
            SlashCommandKind::Extension => format!("x {}", self.name),
            SlashCommandKind::Skill => format!("skill {}", self.name),
            SlashCommandKind::Workflow => format!("workflow {}", self.name),
        }
    }
}

fn merge_namespaced_commands(
    commands: &mut Vec<SlashCommand>,
    kind: SlashCommandKind,
    items: impl IntoIterator<Item = (String, String)>,
    default_description: &'static str,
    description: impl Fn(String) -> String,
) {
    let mut existing: BTreeSet<String> = commands
        .iter()
        .filter(|command| command.kind == kind)
        .map(|command| command.name.clone())
        .collect();
    let mut by_name = BTreeMap::new();

    for (name, item_description) in items {
        by_name.entry(name).or_insert(item_description);
    }

    for (name, item_description) in by_name {
        if !existing.insert(name.clone()) {
            continue;
        }
        commands.push(SlashCommand {
            name,
            kind,
            description: if item_description.trim().is_empty() {
                default_description.into()
            } else {
                description(item_description)
            },
        });
    }
}

/// Merge built-in and extension-provided slash commands for discovery menus.
pub fn merge_extension_commands(
    mut commands: Vec<SlashCommand>,
    extension_commands: impl IntoIterator<Item = (String, String)>,
) -> Vec<SlashCommand> {
    merge_namespaced_commands(
        &mut commands,
        SlashCommandKind::Extension,
        extension_commands,
        "Extension command",
        |description| description,
    );
    commands
}

/// Merge skill entries into the slash menu.
pub fn merge_skill_commands(
    mut commands: Vec<SlashCommand>,
    skills: impl IntoIterator<Item = (String, String)>,
) -> Vec<SlashCommand> {
    merge_namespaced_commands(
        &mut commands,
        SlashCommandKind::Skill,
        skills,
        "Skill",
        |description| format!("Skill: {description}"),
    );
    commands
}

/// Merge workflow entries into the slash menu without overriding real commands.
pub fn merge_workflow_commands(
    mut commands: Vec<SlashCommand>,
    workflows: impl IntoIterator<Item = (String, String)>,
) -> Vec<SlashCommand> {
    merge_namespaced_commands(
        &mut commands,
        SlashCommandKind::Workflow,
        workflows,
        "Workflow",
        |description| format!("Workflow: {description}"),
    );
    commands
}

pub fn builtin_commands() -> Vec<SlashCommand> {
    [
        ("new", "Start a new session"),
        ("resume", "Resume/search sessions"),
        ("model", "Select model"),
        ("compact", "Compact context"),
        ("quit", "Quit"),
        ("loop", "Continue or auto-loop current intent"),
        ("stop", "Stop active work/loop"),
        ("reload", "Reload config and Lua extensions"),
        ("x", "Run a Lua extension command"),
        ("skill", "Run a skill"),
        ("workflow", "Select an active workflow scope"),
        ("setup", "Run setup wizard"),
        ("secrets", "Configure API keys / service secrets"),
        ("login", "OAuth login for Anthropic, OpenAI, or Kimi Code"),
        ("name", "Name current session"),
        ("tree", "Session tree view"),
        ("settings", "Open settings"),
    ]
    .into_iter()
    .map(|(name, description)| SlashCommand {
        kind: SlashCommandKind::Builtin,
        name: name.into(),
        description: description.into(),
    })
    .collect()
}

/// State for the command palette.
#[derive(Debug, Clone)]
pub struct CommandPaletteState {
    pub commands: Vec<SlashCommand>,
    pub filter: String,
    pub selected: usize,
    pub page: CommandPalettePage,
}

impl CommandPaletteState {
    pub fn new(commands: Vec<SlashCommand>) -> Self {
        Self {
            commands,
            filter: String::new(),
            selected: 0,
            page: CommandPalettePage::Commands,
        }
    }

    pub fn filtered(&self) -> Vec<&SlashCommand> {
        let page_commands = self.commands.iter().filter(|c| self.page.includes(c.kind));

        if self.filter.is_empty() {
            page_commands.collect()
        } else {
            let lower = self.filter.to_lowercase();
            let mut results: Vec<(usize, &SlashCommand)> = page_commands
                .filter_map(|c| {
                    let name = c.name.to_lowercase();
                    let invocation = c.invocation().to_lowercase();
                    let desc = c.description.to_lowercase();
                    // Exact prefix gets priority 0, contains gets 1, description match gets 2
                    if name.starts_with(&lower) || invocation.starts_with(&lower) {
                        Some((0, c))
                    } else if name.contains(&lower) || invocation.contains(&lower) {
                        Some((1, c))
                    } else if desc.contains(&lower) {
                        Some((2, c))
                    } else {
                        None
                    }
                })
                .collect();
            results.sort_by_key(|(priority, _)| *priority);
            results.into_iter().map(|(_, c)| c).collect()
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let count = self.filtered().len();
        if self.selected + 1 < count {
            self.selected += 1;
        }
    }

    pub fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    pub fn prev_page(&mut self) {
        self.page = self.page.prev();
        self.selected = 0;
    }

    pub fn next_page(&mut self) {
        self.page = self.page.next();
        self.selected = 0;
    }

    pub fn selected_command(&self) -> Option<&SlashCommand> {
        let filtered = self.filtered();
        filtered.get(self.selected).copied()
    }
}

/// Command palette overlay widget (shown above the editor).
pub struct CommandPaletteView<'a> {
    state: &'a CommandPaletteState,
    theme: &'a Theme,
}

impl<'a> CommandPaletteView<'a> {
    pub fn new(state: &'a CommandPaletteState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for CommandPaletteView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 20 {
            return;
        }

        Clear.render(area, buf);

        let title = if self.state.filter.is_empty() {
            format!(" {} ", self.state.page.title())
        } else {
            format!(" {} /{} ", self.state.page.title(), self.state.filter)
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(self.theme.accent_style());
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = self.state.filtered();
        let total = filtered.len();

        if total == 0 {
            let line = Line::from(Span::styled(
                format!("  No matching {}", self.state.page.label()),
                self.theme.muted_style(),
            ));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        // Find the longest command invocation for alignment
        let max_name_len = filtered
            .iter()
            .map(|c| c.invocation().len())
            .max()
            .unwrap_or(0);

        // Scroll to keep selected visible
        let visible = inner.height as usize;
        let scroll_offset = if self.state.selected >= visible {
            self.state.selected - visible + 1
        } else {
            0
        };
        let show_kind_labels = false;

        for (i, cmd) in filtered.iter().skip(scroll_offset).enumerate() {
            if i >= visible {
                break;
            }

            let abs_idx = scroll_offset + i;
            let is_selected = abs_idx == self.state.selected;

            // Selection indicator
            let indicator = if is_selected { " ▸ " } else { "   " };

            // Build the command invocation with / prefix, padded for alignment
            let invocation = cmd.invocation();
            let name_text = format!("/{:<width$}", invocation, width = max_name_len);

            // Build the line with full-row highlight when selected
            let row_style = if is_selected {
                self.theme.selected_style()
            } else {
                Style::default()
            };

            let name_style = if is_selected {
                self.theme.selected_style().add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };

            let desc_style = if is_selected {
                self.theme.selected_style()
            } else {
                self.theme.muted_style()
            };

            let kind_text = if show_kind_labels {
                format!(" [{:<10}]", cmd.kind.label())
            } else {
                String::new()
            };

            let line = Line::from(vec![
                Span::styled(indicator, row_style),
                Span::styled(name_text, name_style),
                Span::styled(kind_text, desc_style),
                Span::styled("  ", row_style),
                Span::styled(&cmd.description, desc_style),
            ]);

            // Fill the entire row with the background color first
            if is_selected {
                let fill = " ".repeat(inner.width as usize);
                buf.set_line(
                    inner.x,
                    inner.y + i as u16,
                    &Line::from(Span::styled(fill, row_style)),
                    inner.width,
                );
            }

            buf.set_line(inner.x, inner.y + i as u16, &line, inner.width);
        }

        // Scroll indicators
        if scroll_offset > 0 {
            let hint = Line::from(Span::styled("  ↑ more", self.theme.muted_style()));
            buf.set_line(inner.x + inner.width.saturating_sub(10), inner.y, &hint, 10);
        }
        if scroll_offset + visible < total {
            let y = inner.y + inner.height.saturating_sub(1);
            let hint = Line::from(Span::styled("  ↓ more", self.theme.muted_style()));
            buf.set_line(inner.x + inner.width.saturating_sub(10), y, &hint, 10);
        }

        // Footer hint
        if inner.height > 1 && total > 0 {
            let hint_y = area.y + area.height - 1;
            let hint_text = " ←→ page  ↑↓ item  Enter  Esc ";
            let hint_x = area.x + area.width.saturating_sub(hint_text.len() as u16 + 1);
            let hint_line = Line::from(Span::styled(hint_text, self.theme.muted_style()));
            buf.set_line(hint_x, hint_y, &hint_line, hint_text.len() as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, kind: SlashCommandKind) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: format!("{name} description"),
            kind,
        }
    }

    #[test]
    fn palette_pages_partition_commands_extensions_skills_and_workflows() {
        let mut state = CommandPaletteState::new(vec![
            command("new", SlashCommandKind::Builtin),
            command("greet", SlashCommandKind::Extension),
            command("rust", SlashCommandKind::Skill),
            command("ship", SlashCommandKind::Workflow),
        ]);

        let names: Vec<&str> = state
            .filtered()
            .iter()
            .map(|cmd| cmd.name.as_str())
            .collect();
        assert_eq!(names, vec!["new"]);

        state.next_page();
        let names: Vec<&str> = state
            .filtered()
            .iter()
            .map(|cmd| cmd.name.as_str())
            .collect();
        assert_eq!(names, vec!["greet"]);

        state.next_page();
        let names: Vec<&str> = state
            .filtered()
            .iter()
            .map(|cmd| cmd.name.as_str())
            .collect();
        assert_eq!(names, vec!["rust"]);

        state.next_page();
        let names: Vec<&str> = state
            .filtered()
            .iter()
            .map(|cmd| cmd.name.as_str())
            .collect();
        assert_eq!(names, vec!["ship"]);
    }

    #[test]
    fn palette_filter_applies_to_active_page_only() {
        let mut state = CommandPaletteState::new(vec![
            command("review", SlashCommandKind::Builtin),
            command("review", SlashCommandKind::Skill),
            command("review-flow", SlashCommandKind::Workflow),
        ]);
        state.push_filter('r');
        state.push_filter('e');

        assert_eq!(state.filtered().len(), 1);
        assert_eq!(
            state.selected_command().unwrap().kind,
            SlashCommandKind::Builtin
        );

        state.next_page();
        assert!(state.filtered().is_empty());

        state.next_page();
        assert_eq!(state.filtered().len(), 1);
        assert_eq!(
            state.selected_command().unwrap().kind,
            SlashCommandKind::Skill
        );
    }

    #[test]
    fn namespaced_commands_render_canonical_invocations() {
        assert_eq!(
            command("new", SlashCommandKind::Builtin).invocation(),
            "new"
        );
        assert_eq!(
            command("greet", SlashCommandKind::Extension).invocation(),
            "x greet"
        );
        assert_eq!(
            command("rust", SlashCommandKind::Skill).invocation(),
            "skill rust"
        );
        assert_eq!(
            command("ship", SlashCommandKind::Workflow).invocation(),
            "workflow ship"
        );
    }
}
