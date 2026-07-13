use imp_llm::truncate_chars_with_suffix;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme::Theme;

/// A flattened tree node for display.
#[derive(Debug, Clone)]
pub struct FlatTreeNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub guides: Vec<bool>,
    pub summary: String,
    pub full_text: String,
    pub kind_label: &'static str,
    pub is_user: bool,
    pub is_tool: bool,
    pub is_compaction: bool,
    pub has_children: bool,
    pub child_count: usize,
    pub is_last_child: bool,
}

/// Filter mode for the tree view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeFilterMode {
    All,
    CurrentBranch,
    BranchPoints,
    NoTools,
    UserOnly,
}

impl TreeFilterMode {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::CurrentBranch,
            Self::CurrentBranch => Self::BranchPoints,
            Self::BranchPoints => Self::NoTools,
            Self::NoTools => Self::UserOnly,
            Self::UserOnly => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::CurrentBranch => "current-branch",
            Self::BranchPoints => "branch-points",
            Self::NoTools => "no-tools",
            Self::UserOnly => "user-only",
        }
    }
}

/// State for the tree view overlay.
#[derive(Debug, Clone)]
pub struct TreeViewState {
    pub nodes: Vec<FlatTreeNode>,
    pub filter_mode: TreeFilterMode,
    pub current_id: Option<String>,
    selected_id: Option<String>,
}

impl TreeViewState {
    pub fn new(nodes: Vec<FlatTreeNode>, current_id: Option<String>) -> Self {
        let selected_id = nodes.last().map(|n| n.id.clone());

        Self {
            nodes,
            filter_mode: TreeFilterMode::All,
            current_id,
            selected_id,
        }
    }

    pub fn move_up(&mut self) {
        let filtered = self.filtered_nodes();
        if filtered.is_empty() {
            self.selected_id = None;
            return;
        }

        let idx = self.selected_filtered_index_in(&filtered).unwrap_or(0);
        let next = idx.saturating_sub(1);
        self.selected_id = Some(filtered[next].id.clone());
    }

    pub fn move_down(&mut self) {
        let filtered = self.filtered_nodes();
        if filtered.is_empty() {
            self.selected_id = None;
            return;
        }

        let idx = self.selected_filtered_index_in(&filtered).unwrap_or(0);
        let next = (idx + 1).min(filtered.len().saturating_sub(1));
        self.selected_id = Some(filtered[next].id.clone());
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected_id.as_deref()
    }

    pub fn selected_node(&self) -> Option<&FlatTreeNode> {
        let id = self.selected_id.as_deref()?;
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn cycle_filter(&mut self) {
        self.filter_mode = self.filter_mode.next();
        self.reanchor_selection();
    }

    pub fn filtered_nodes(&self) -> Vec<&FlatTreeNode> {
        let current_branch_ids = self.current_branch_ids();
        self.nodes
            .iter()
            .rev()
            .filter(|n| match self.filter_mode {
                TreeFilterMode::All => true,
                TreeFilterMode::CurrentBranch => current_branch_ids.contains(n.id.as_str()),
                TreeFilterMode::BranchPoints => n.child_count > 1,
                TreeFilterMode::NoTools => !n.is_tool,
                TreeFilterMode::UserOnly => n.is_user,
            })
            .collect()
    }

    pub fn selected_filtered_index(&self) -> Option<usize> {
        let filtered = self.filtered_nodes();
        self.selected_filtered_index_in(&filtered)
    }

    fn reanchor_selection(&mut self) {
        let filtered = self.filtered_nodes();
        if filtered.is_empty() {
            self.selected_id = None;
            return;
        }

        if self.selected_filtered_index_in(&filtered).is_some() {
            return;
        }

        if let Some(current_id) = self.current_id.as_deref() {
            if filtered.iter().any(|node| node.id == current_id) {
                self.selected_id = Some(current_id.to_string());
                return;
            }
        }

        self.selected_id = filtered.first().map(|node| node.id.clone());
    }

    fn selected_filtered_index_in(&self, filtered: &[&FlatTreeNode]) -> Option<usize> {
        let selected_id = self.selected_id.as_deref()?;
        filtered.iter().position(|node| node.id == selected_id)
    }

    fn current_branch_ids(&self) -> std::collections::HashSet<&str> {
        let mut ids = std::collections::HashSet::new();
        let mut current = self.current_id.as_deref();

        while let Some(id) = current {
            if !ids.insert(id) {
                break;
            }
            current = self
                .nodes
                .iter()
                .find(|node| node.id == id)
                .and_then(|node| node.parent_id.as_deref());
        }

        ids
    }
}

/// Flatten a session tree into displayable nodes.
pub fn flatten_tree(tree: &[imp_core::session::TreeNode], depth: usize) -> Vec<FlatTreeNode> {
    let mut result = Vec::new();
    flatten_tree_into(tree, depth, Vec::new(), &mut result);
    result
}

fn flatten_tree_into(
    tree: &[imp_core::session::TreeNode],
    depth: usize,
    guides: Vec<bool>,
    out: &mut Vec<FlatTreeNode>,
) {
    let len = tree.len();

    for (i, node) in tree.iter().enumerate() {
        let has_more_siblings = i + 1 < len;

        match &node.entry {
            imp_core::session::SessionEntry::Message {
                id,
                parent_id,
                message,
            } => {
                let text = extract_text(message);
                let full_text = fallback_message_text(message, text);
                let truncated = truncate_chars_with_suffix(&full_text, 57, "…");
                let is_user = message.is_user();
                let is_tool = message.is_tool_result();
                out.push(FlatTreeNode {
                    id: id.clone(),
                    parent_id: parent_id.clone(),
                    depth,
                    guides: guides.clone(),
                    summary: truncated,
                    full_text,
                    kind_label: if is_user {
                        "user"
                    } else if is_tool {
                        "tool"
                    } else {
                        "assistant"
                    },
                    is_user,
                    is_tool,
                    is_compaction: false,
                    has_children: !node.children.is_empty(),
                    child_count: node.children.len(),
                    is_last_child: !has_more_siblings,
                });
            }
            imp_core::session::SessionEntry::CompactionV2 {
                id,
                parent_id,
                record,
            } => {
                let full_text = record.summary.trim().to_string();
                out.push(FlatTreeNode {
                    id: id.clone(),
                    parent_id: parent_id.clone(),
                    depth,
                    guides: guides.clone(),
                    summary: format!("[compaction-v2: {}]", truncate(&record.summary, 40)),
                    full_text,
                    kind_label: "compaction",
                    is_user: false,
                    is_tool: false,
                    is_compaction: true,
                    has_children: !node.children.is_empty(),
                    child_count: node.children.len(),
                    is_last_child: !has_more_siblings,
                });
            }
            imp_core::session::SessionEntry::Compaction {
                id,
                parent_id,
                summary,
                ..
            } => {
                let full_text = summary.trim().to_string();
                out.push(FlatTreeNode {
                    id: id.clone(),
                    parent_id: parent_id.clone(),
                    depth,
                    guides: guides.clone(),
                    summary: format!("[compaction: {}]", truncate(summary, 40)),
                    full_text: if full_text.is_empty() {
                        "(compaction summary)".to_string()
                    } else {
                        full_text
                    },
                    kind_label: "compaction",
                    is_user: false,
                    is_tool: false,
                    is_compaction: true,
                    has_children: !node.children.is_empty(),
                    child_count: node.children.len(),
                    is_last_child: !has_more_siblings,
                });
            }
            _ => {}
        }

        if !node.children.is_empty() {
            let mut child_guides = guides.clone();
            child_guides.push(has_more_siblings);
            flatten_tree_into(&node.children, depth + 1, child_guides, out);
        }
    }
}

/// Session tree navigator overlay.
pub struct TreeView<'a> {
    state: &'a TreeViewState,
    theme: &'a Theme,
}

impl<'a> TreeView<'a> {
    pub fn new(state: &'a TreeViewState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for TreeView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        Clear.render(area, buf);
        let filtered = self.state.filtered_nodes();
        let selected_index = self.state.selected_filtered_index().unwrap_or(0) + 1;
        let title = if filtered.is_empty() {
            format!(
                " Session Tree · {} · newest first · 0/0 ",
                self.state.filter_mode.label()
            )
        } else {
            format!(
                " Session Tree · {} · newest first · {}/{} ",
                self.state.filter_mode.label(),
                selected_index,
                filtered.len()
            )
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(self.theme.border_style());
        let inner = block.inner(area);
        block.render(area, buf);

        if filtered.is_empty() {
            let line = Line::from(Span::styled(
                "  No matching nodes",
                self.theme.muted_style(),
            ));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        let has_preview = inner.width >= 140 && inner.height >= 18;
        let columns = if has_preview {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(100), Constraint::Percentage(0)])
                .split(inner)
        };

        render_tree_list(
            columns[0],
            self.state,
            &filtered,
            buf,
            self.theme,
            has_preview,
        );
        if has_preview {
            render_tree_preview(columns[1], self.state.selected_node(), buf, self.theme);
        }
    }
}

fn render_tree_list(
    area: Rect,
    state: &TreeViewState,
    filtered: &[&FlatTreeNode],
    buf: &mut Buffer,
    theme: &Theme,
    show_tree_shape: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let selected_idx = state.selected_filtered_index().unwrap_or(0);
    let visible_height = area.height as usize;
    let scroll = if selected_idx >= visible_height {
        selected_idx - visible_height + 1
    } else {
        0
    };

    for (row, node) in filtered
        .iter()
        .skip(scroll)
        .take(visible_height)
        .enumerate()
    {
        let y = area.y + row as u16;
        let is_selected = scroll + row == selected_idx;
        let is_current = state.current_id.as_deref() == Some(&node.id);

        let marker = if is_current { "●" } else { " " };
        let (indent, branch) = if show_tree_shape {
            tree_prefix(node)
        } else if node.depth == 0 {
            (String::new(), String::new())
        } else {
            ("·".repeat(node.depth.min(8)), " ".to_string())
        };
        let kind = kind_tag(node);
        let suffix = branch_suffix(node);

        let prefix_len = marker.chars().count()
            + 1
            + indent.chars().count()
            + branch.chars().count()
            + kind.chars().count()
            + 1;
        let suffix_len = suffix.chars().count();
        let summary_width = area.width.saturating_sub((prefix_len + suffix_len) as u16) as usize;
        let summary = truncate_chars_with_suffix(&node.summary, summary_width.max(8), "…");

        let style = if is_selected {
            theme.selected_style()
        } else if is_current {
            theme.accent_style()
        } else if node.is_tool {
            theme.muted_style()
        } else if node.is_compaction {
            theme.accent_style()
        } else {
            Style::default()
        };

        let line = Line::from(vec![
            Span::styled(marker.to_string(), theme.accent_style()),
            Span::raw(" "),
            Span::styled(indent, theme.muted_style()),
            Span::styled(branch, theme.muted_style()),
            Span::styled(kind, Style::default().add_modifier(Modifier::DIM)),
            Span::raw(" "),
            Span::styled(summary, style),
            Span::styled(suffix, theme.muted_style()),
        ]);

        buf.set_line(area.x, y, &line, area.width);
    }

    if scroll > 0 {
        let indicator = Line::from(Span::styled("▲", theme.muted_style()));
        buf.set_line(area.x + area.width.saturating_sub(1), area.y, &indicator, 1);
    }
    if scroll + visible_height < filtered.len() {
        let indicator = Line::from(Span::styled("▼", theme.muted_style()));
        buf.set_line(
            area.x + area.width.saturating_sub(1),
            area.y + area.height.saturating_sub(1),
            &indicator,
            1,
        );
    }
}

fn render_tree_preview(area: Rect, node: Option<&FlatTreeNode>, buf: &mut Buffer, theme: &Theme) {
    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::LEFT)
        .border_style(theme.muted_style());
    let inner = block.inner(area);
    block.render(area, buf);

    let Some(node) = node else {
        return;
    };

    let lines = [
        format!("Kind: {}", node.kind_label),
        format!("ID: {}", node.id),
        format!("Parent: {}", node.parent_id.as_deref().unwrap_or("(root)")),
        format!("Depth: {}", node.depth),
        format!("Children: {}", node.child_count),
        format!(
            "Branching: {}",
            if node.child_count > 1 {
                "branch point"
            } else if node.has_children {
                "continues"
            } else {
                "leaf"
            }
        ),
        String::new(),
        "Content:".to_string(),
        node.full_text.clone(),
        String::new(),
        "Enter: checkout • f: fork • Ctrl+O: filter all/current/branches/no-tools/user".to_string(),
    ];

    let wrapped = wrap_lines(&lines, inner.width as usize, inner.height as usize);
    for (i, line) in wrapped.iter().enumerate() {
        if i >= inner.height as usize {
            break;
        }
        let style = if line.is_empty() {
            theme.muted_style()
        } else if line == "Content:" {
            theme.accent_style()
        } else {
            theme.muted_style()
        };
        let rendered = Line::from(Span::styled(line.clone(), style));
        buf.set_line(inner.x, inner.y + i as u16, &rendered, inner.width);
    }
}

fn branch_suffix(node: &FlatTreeNode) -> String {
    if node.child_count > 1 {
        format!(" +{}", node.child_count)
    } else if node.has_children {
        " ›".to_string()
    } else {
        String::new()
    }
}

fn tree_prefix(node: &FlatTreeNode) -> (String, String) {
    if node.depth == 0 {
        return (String::new(), String::new());
    }

    let visible_depth = node.guides.len().min(6);
    let hidden_depth = node.guides.len().saturating_sub(visible_depth);
    let mut indent = String::new();
    if hidden_depth > 0 {
        indent.push('…');
    }
    for guide in &node.guides[hidden_depth..] {
        indent.push_str(if *guide { "│" } else { " " });
    }

    let branch = if node.is_last_child { "└" } else { "├" }.to_string();
    (indent, branch)
}
fn kind_tag(node: &FlatTreeNode) -> &'static str {
    if node.is_compaction {
        "C"
    } else if node.is_user {
        "U"
    } else if node.is_tool {
        "T"
    } else {
        "A"
    }
}

fn fallback_message_text(msg: &imp_llm::Message, text: String) -> String {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    match msg {
        imp_llm::Message::User(_) => "(user message)".to_string(),
        imp_llm::Message::Assistant(_) => "(assistant message)".to_string(),
        imp_llm::Message::ToolResult(_) => "(tool result)".to_string(),
    }
}

fn extract_text(msg: &imp_llm::Message) -> String {
    let blocks = match msg {
        imp_llm::Message::User(u) => &u.content,
        imp_llm::Message::Assistant(a) => &a.content,
        imp_llm::Message::ToolResult(t) => &t.content,
    };
    blocks
        .iter()
        .find_map(|b| match b {
            imp_llm::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    truncate_chars_with_suffix(s, max.saturating_sub(1), "…")
}

fn wrap_lines(lines: &[String], width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for line in lines {
        if line.is_empty() {
            out.push(String::new());
            if out.len() >= max_lines {
                break;
            }
            continue;
        }

        let mut current = String::new();
        for word in line.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };

            if candidate.chars().count() <= width {
                current = candidate;
            } else {
                if !current.is_empty() {
                    out.push(current);
                    if out.len() >= max_lines {
                        return out;
                    }
                }
                current = truncate_chars_with_suffix(word, width, "…");
            }
        }

        if !current.is_empty() {
            out.push(current);
            if out.len() >= max_lines {
                break;
            }
        }
    }

    out.truncate(max_lines);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &'static str) -> FlatTreeNode {
        FlatTreeNode {
            id: id.to_string(),
            parent_id: None,
            depth: 0,
            guides: Vec::new(),
            summary: id.to_string(),
            full_text: id.to_string(),
            kind_label: kind,
            is_user: kind == "user",
            is_tool: kind == "tool",
            is_compaction: kind == "compaction",
            has_children: false,
            child_count: 0,
            is_last_child: true,
        }
    }

    #[test]
    fn current_branch_filter_keeps_only_current_ancestry() {
        let mut nodes = vec![
            node("root", "user"),
            node("left", "assistant"),
            node("right", "assistant"),
        ];
        nodes[1].parent_id = Some("root".into());
        nodes[2].parent_id = Some("root".into());

        let mut state = TreeViewState::new(nodes, Some("left".into()));
        state.cycle_filter(); // current-branch

        let ids: Vec<&str> = state
            .filtered_nodes()
            .into_iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(ids, vec!["left", "root"]);
    }

    #[test]
    fn branch_points_filter_only_shows_multi_child_nodes() {
        let mut nodes = vec![
            node("root", "user"),
            node("mid", "assistant"),
            node("leaf", "assistant"),
        ];
        nodes[0].has_children = true;
        nodes[0].child_count = 2;
        nodes[1].has_children = true;
        nodes[1].child_count = 1;

        let mut state = TreeViewState::new(nodes, Some("leaf".into()));
        state.cycle_filter(); // current-branch
        state.cycle_filter(); // branch-points

        let ids: Vec<&str> = state
            .filtered_nodes()
            .into_iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(ids, vec!["root"]);
    }

    #[test]
    fn selection_is_preserved_when_filter_keeps_same_node_visible() {
        let nodes = vec![
            node("u1", "user"),
            node("a1", "assistant"),
            node("t1", "tool"),
        ];
        let mut state = TreeViewState::new(nodes, Some("a1".into()));

        state.cycle_filter(); // current-branch
        state.cycle_filter(); // branch-points
        state.cycle_filter(); // no-tools

        assert_eq!(state.selected_id(), Some("a1"));
    }

    #[test]
    fn filter_fallback_prefers_current_id_when_selected_node_disappears() {
        let nodes = vec![
            node("u1", "user"),
            node("a1", "assistant"),
            node("t1", "tool"),
        ];
        let mut state = TreeViewState::new(nodes, Some("t1".into()));
        assert_eq!(state.selected_id(), Some("t1"));

        state.cycle_filter(); // current-branch
        state.cycle_filter(); // branch-points
        state.cycle_filter(); // no-tools

        assert_eq!(state.selected_id(), Some("a1"));
    }

    #[test]
    fn render_guides_draws_vertical_connectors_for_open_ancestors() {
        let node = FlatTreeNode {
            id: "child".into(),
            parent_id: Some("parent".into()),
            depth: 3,
            guides: vec![true, false, true],
            summary: "child".into(),
            full_text: "child".into(),
            kind_label: "assistant",
            is_user: false,
            is_tool: false,
            is_compaction: false,
            has_children: false,
            child_count: 0,
            is_last_child: false,
        };
        assert_eq!(tree_prefix(&node), ("│ │".to_string(), "├".to_string()));
    }
}
