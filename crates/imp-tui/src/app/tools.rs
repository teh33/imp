use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::views::chat::visible_line_window;
use crate::views::tools::DisplayToolCall;

use super::{open_path_in_editor, selected_read_file_path_from_tool, App, Pane};

impl App {
    pub(super) fn selected_tool_call(&self) -> Option<DisplayToolCall> {
        let index = match self.tool_focus {
            Some(index) => index,
            None if self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Inspector => {
                self.total_tool_calls().checked_sub(1)?
            }
            None => return None,
        };

        self.messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .nth(index)
            .cloned()
    }

    /// Focus a tool call by flat index: update tool_focus and sync sidebar.
    pub(super) fn focus_tool(&mut self, index: usize) {
        self.focus_tool_with_pin(index, true);
    }

    /// Focus a tool call from an inline chat hit and show its details immediately.
    pub(super) fn focus_tool_in_inspector(&mut self, index: usize) {
        self.focus_tool_with_pin(index, true);
        self.active_pane = Pane::SidebarDetail;
    }

    pub(super) fn focus_latest_tool_with_pin(&mut self, pinned: bool) -> bool {
        let total = self.total_tool_calls();
        if total == 0 {
            return false;
        }
        self.focus_tool_with_pin(total - 1, pinned);
        true
    }

    pub(super) fn focus_tool_with_pin(&mut self, index: usize, pinned: bool) {
        self.tool_focus = Some(index);
        self.tool_focus_pinned = pinned;
        self.sidebar_auto_follow = !pinned;
        self.sidebar.open = true;
        self.sidebar.reset_detail_scroll();
        self.active_pane = match self.config.ui.sidebar_style {
            imp_core::config::SidebarStyle::Split => Pane::SidebarList,
            imp_core::config::SidebarStyle::Inspector | imp_core::config::SidebarStyle::Stream => {
                Pane::SidebarDetail
            }
        };
        if self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Split {
            self.sidebar.ensure_selected_visible(index);
        }
    }

    pub(super) fn selected_read_file_path(&self) -> Option<PathBuf> {
        selected_read_file_path_from_tool(self.selected_tool_call().as_ref(), &self.cwd)
    }

    pub(super) fn open_selected_read_file(&mut self) {
        let Some(path) = self.selected_read_file_path() else {
            self.push_system_msg("No read file selected to open.");
            return;
        };

        if !path.is_file() {
            self.push_error_msg(&format!(
                "Selected read file does not exist: {}",
                path.display()
            ));
            return;
        }

        match open_path_in_editor(&path) {
            Ok(()) => self.push_system_msg(&format!("Opened {}", path.display())),
            Err(error) => {
                self.push_error_msg(&format!("Failed to open {}: {error}", path.display()))
            }
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        if self.sidebar.open {
            self.sidebar.open = false;
            self.active_pane = Pane::Chat;
        } else {
            self.sidebar.open = true;
            if self.tool_focus.is_none() && !self.focus_latest_tool_with_pin(false) {
                self.active_pane = Pane::Chat;
            } else {
                self.active_pane = Pane::SidebarDetail;
            }
        }
    }

    pub(super) fn tool_id_at_chat_row(&self, row: u16, chat_area: Rect) -> Option<String> {
        if row < chat_area.y || row >= chat_area.y.saturating_add(chat_area.height) {
            return None;
        }

        if let Some(tool_id) = self
            .chat_tool_click_map
            .iter()
            .find_map(|(tool_row, tool_id)| (*tool_row == row).then(|| tool_id.clone()))
        {
            return Some(tool_id);
        }

        // Fall back to the render data's tool-line indices. The cached click map is
        // derived from rendered text for selection support, but styling/format
        // changes can make text parsing miss valid tool headers. The render data is
        // the authoritative source of which visual chat lines belong to tool calls.
        let render = self.chat_render_cache.as_ref().map(|cache| &cache.render)?;
        let total_lines = render.lines.len();
        let window =
            visible_line_window(total_lines, chat_area.height as usize, self.scroll_offset);
        let line_index = window.start + (row - chat_area.y) as usize;
        render
            .tool_line_indices
            .iter()
            .find_map(|(tool_line, tool_id)| (*tool_line == line_index).then(|| tool_id.clone()))
    }

    /// Total number of tool calls across all display messages.
    pub(super) fn total_tool_calls(&self) -> usize {
        self.messages.iter().map(|m| m.tool_calls.len()).sum()
    }

    /// Mutable access to a tool call by its flat index across all messages.
    pub(super) fn get_tool_call_mut(
        &mut self,
        flat_idx: usize,
    ) -> Option<&mut crate::views::tools::DisplayToolCall> {
        let mut remaining = flat_idx;
        for msg in &mut self.messages {
            if remaining < msg.tool_calls.len() {
                return Some(&mut msg.tool_calls[remaining]);
            }
            remaining -= msg.tool_calls.len();
        }
        None
    }
}
