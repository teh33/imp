use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};

use crate::keybindings::{self, Action};
use crate::views::command_palette::CommandPaletteState;

use super::{
    extract_selected_text, point_in_rect, App, DragAutoScroll, Pane, QueuedMessage,
    ScrollDirection, SelectablePane, SelectionState, TextSurface, UiMode, WelcomeStep,
};

impl App {
    // ── Key handling ────────────────────────────────────────────

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Result<(), Box<dyn std::error::Error>> {
        self.needs_redraw = true;

        if self.ask_state.is_some() && self.is_paste_shortcut(key) {
            self.paste_from_clipboard();
            return Ok(());
        }

        // Reset ctrl+c counter on non-ctrl+c keypress
        if !(key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
            self.ctrl_c_count = 0;
        }

        // Ask overlay intercepts all keys when active
        if self.ask_state.is_some() {
            self.handle_ask_key(key);
            return Ok(());
        }

        // Route based on current UI mode
        match &self.mode {
            UiMode::Normal => self.handle_normal_key(key)?,
            UiMode::ModelSelector(_)
            | UiMode::CommandPalette(_)
            | UiMode::LoginPicker(_)
            | UiMode::SecretsPicker(_) => self.handle_overlay_key(key),
            #[cfg(feature = "mana-ui")]
            UiMode::ManaNavigator(_) => self.handle_mana_navigator_key(key),
            UiMode::TreeView(_) => self.handle_tree_key(key),
            UiMode::Settings(_) => self.handle_settings_key(key),
            UiMode::SessionPicker(_) => self.handle_session_picker_key(key),
            UiMode::Welcome(_) => self.handle_welcome_key(key),
        }

        Ok(())
    }

    pub(super) fn handle_normal_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_copy_shortcut(key) {
            let _ = self.copy_selection();
            return Ok(());
        }
        if self.is_paste_shortcut(key) {
            self.paste_from_clipboard();
            return Ok(());
        }

        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    if self.extend_selection_lines(-1) {
                        return Ok(());
                    }
                }
                KeyCode::Down => {
                    if self.extend_selection_lines(1) {
                        return Ok(());
                    }
                }
                KeyCode::PageUp => {
                    if self.extend_selection_lines(-(self.config.ui.keyboard_scroll_lines as isize))
                    {
                        return Ok(());
                    }
                }
                KeyCode::PageDown => {
                    if self.extend_selection_lines(self.config.ui.keyboard_scroll_lines as isize) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        if key.code == KeyCode::Esc && self.selection.is_some() {
            self.clear_selection();
            return Ok(());
        }

        let action = keybindings::resolve_normal(key);

        match action {
            Some(Action::Submit) => {
                if self.is_streaming {
                    let text = self.editor.content().to_string();
                    if !text.trim().is_empty() {
                        self.queue_streaming_message(QueuedMessage::Steer(text));
                    }
                } else {
                    self.send_message();
                }
            }
            Some(Action::FollowUp) => {
                if self.is_streaming {
                    let text = self.editor.content().to_string();
                    if !text.trim().is_empty() {
                        self.queue_streaming_message(QueuedMessage::FollowUp(text));
                    }
                }
            }
            Some(Action::NewLine) => {
                self.editor.insert_newline();
            }
            Some(Action::Cancel) => {
                self.handle_cancel();
            }
            Some(Action::SelectModel) => {
                self.open_model_selector();
            }
            Some(Action::CycleModelForward) => {
                self.cycle_model(true);
            }
            Some(Action::CycleModelBackward) => {
                self.cycle_model(false);
            }
            Some(Action::CycleThinking) => {
                self.cycle_thinking_level();
            }
            Some(Action::SidebarToggle) => {
                self.toggle_sidebar();
            }
            Some(Action::Peek) => {
                // Legacy alias — behaves the same as ToolToggle with no focus
                self.tools_expanded = !self.tools_expanded;
                for msg in &mut self.messages {
                    for tc in &mut msg.tool_calls {
                        tc.expanded = self.tools_expanded;
                    }
                }
                self.invalidate_chat_render_cache();
                self.needs_redraw = true;
            }
            Some(Action::OpenSelectedReadFile) => {
                self.open_selected_read_file();
            }
            Some(Action::ToolToggle) => {
                if let Some(idx) = self.tool_focus {
                    // Toggle just the focused tool call
                    if let Some(tc) = self.get_tool_call_mut(idx) {
                        tc.expanded = !tc.expanded;
                    }
                    self.invalidate_chat_render_cache();
                } else {
                    // No focus: toggle all (global expand/collapse)
                    self.tools_expanded = !self.tools_expanded;
                    for msg in &mut self.messages {
                        for tc in &mut msg.tool_calls {
                            tc.expanded = self.tools_expanded;
                        }
                    }
                    self.invalidate_chat_render_cache();
                }
            }
            Some(Action::ToolFocusNext) => {
                let total = self.total_tool_calls();
                if total > 0 {
                    if !self.sidebar.open {
                        self.sidebar.open = true;
                        self.focus_latest_tool_with_pin(false);
                    } else {
                        let idx = match self.tool_focus {
                            None => 0,
                            Some(i) => (i + 1).min(total - 1),
                        };
                        self.focus_tool(idx);
                    }
                }
            }
            Some(Action::ToolFocusPrev) => {
                let total = self.total_tool_calls();
                if total > 0 {
                    if !self.sidebar.open {
                        self.sidebar.open = true;
                        self.focus_latest_tool_with_pin(false);
                    } else {
                        let idx = match self.tool_focus {
                            None => total.saturating_sub(1),
                            Some(i) => i.saturating_sub(1),
                        };
                        self.focus_tool(idx);
                    }
                }
            }
            Some(Action::InsertChar('/')) if self.editor.is_empty() && !self.is_streaming => {
                self.editor.insert_char('/');
                self.mode = UiMode::CommandPalette(CommandPaletteState::new(self.slash_commands()));
            }
            Some(Action::InsertChar(c)) => {
                self.editor.insert_char(c);
            }
            Some(Action::Backspace) => {
                self.editor.delete_back();
            }
            Some(Action::Delete) => {
                self.editor.delete_forward();
            }
            Some(Action::CursorLeft) => {
                self.editor.move_left();
            }
            Some(Action::CursorRight) => {
                self.editor.move_right();
            }
            Some(Action::CursorUp) => {
                if self.sidebar.open && self.active_pane == Pane::SidebarList {
                    let total = self.total_tool_calls();
                    if total > 0 {
                        let idx = match self.tool_focus {
                            None => total.saturating_sub(1),
                            Some(i) => i.saturating_sub(1),
                        };
                        self.focus_tool(idx);
                    }
                } else if !self.editor.move_up() {
                    self.editor.history_prev();
                }
            }
            Some(Action::CursorDown) => {
                if self.sidebar.open && self.active_pane == Pane::SidebarList {
                    let total = self.total_tool_calls();
                    if total > 0 {
                        let idx = match self.tool_focus {
                            None => 0,
                            Some(i) => (i + 1).min(total - 1),
                        };
                        self.focus_tool(idx);
                    }
                } else if !self.editor.move_down() {
                    self.editor.history_next();
                }
            }
            Some(Action::CursorHome) => {
                self.editor.move_home();
            }
            Some(Action::CursorEnd) => {
                self.editor.move_end();
            }
            Some(Action::WordLeft) => {
                self.editor.move_word_left();
            }
            Some(Action::WordRight) => {
                self.editor.move_word_right();
            }
            Some(Action::DeleteWordBack) => {
                self.editor.delete_word_back();
            }
            Some(Action::DeleteToStart) => {
                self.editor.delete_to_start();
            }
            Some(Action::DeleteToEnd) => {
                self.editor.delete_to_end();
            }
            Some(Action::ScrollUp) | Some(Action::PageUp) => {
                self.scroll_active_pane_up(self.config.ui.keyboard_scroll_lines);
            }
            Some(Action::ScrollDown) | Some(Action::PageDown) => {
                self.scroll_active_pane_down(self.config.ui.keyboard_scroll_lines);
            }
            Some(Action::Quit) => {
                self.handle_cancel();
            }
            _ => {}
        }

        Ok(())
    }

    pub(super) fn scroll_chat_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        self.auto_scroll = false;
    }

    pub(super) fn scroll_chat_down(&mut self, lines: usize) {
        if self.streaming_anchor_user_index.is_some() {
            self.streaming_anchor_user_index = None;
            self.auto_scroll = false;
        }

        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }
    }

    pub(super) fn sidebar_mouse_scroll_lines(&self) -> usize {
        1
    }

    pub(super) fn scroll_active_pane_up(&mut self, lines: usize) {
        match self.active_pane {
            Pane::SidebarList if self.sidebar.open => self.sidebar.scroll_list_up(lines),
            Pane::SidebarDetail if self.sidebar.open => {
                self.sidebar_auto_follow = false;
                self.sidebar.scroll_detail_up(lines);
            }
            _ => self.scroll_chat_up(lines),
        }
    }

    pub(super) fn scroll_active_pane_down(&mut self, lines: usize) {
        match self.active_pane {
            Pane::SidebarList if self.sidebar.open => self.sidebar.scroll_list_down(lines),
            Pane::SidebarDetail if self.sidebar.open => {
                self.sidebar_auto_follow = false;
                self.sidebar.scroll_detail_down(lines);
            }
            _ => self.scroll_chat_down(lines),
        }
    }

    pub(super) fn selection_surface(&self, pane: SelectablePane) -> Option<&TextSurface> {
        match pane {
            SelectablePane::Chat => self.chat_surface.as_ref(),
            SelectablePane::SidebarDetail => self.sidebar_detail_surface.as_ref(),
        }
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = None;
        self.drag_selection = None;
        self.drag_autoscroll = None;
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let surface = self.selection_surface(selection.pane)?;
        extract_selected_text(surface, selection).filter(|text| !text.is_empty())
    }

    pub(super) fn copy_to_clipboard(&self, text: &str) {
        #[cfg(target_os = "macos")]
        {
            let _ = Self::write_to_clipboard_command("pbcopy", &[], text);
        }
        #[cfg(target_os = "linux")]
        {
            let _ = Self::write_to_clipboard_linux(text);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn write_to_clipboard_command(program: &str, args: &[&str], text: &str) -> bool {
        use std::io::Write;

        let Ok(mut child) = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return false;
        };

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                return false;
            }
        }

        child.wait().is_ok_and(|status| status.success())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn write_to_clipboard_linux(text: &str) -> bool {
        Self::write_to_clipboard_command("wl-copy", &[], text)
            || Self::write_to_clipboard_command("xclip", &["-selection", "clipboard"], text)
            || Self::write_to_clipboard_command("xsel", &["--clipboard", "--input"], text)
    }

    pub(super) fn copy_selection(&mut self) -> bool {
        if let Some(text) = self.selection_text() {
            self.copy_to_clipboard(&text);
            self.push_system_msg("Copied selection to clipboard.");
            true
        } else {
            false
        }
    }

    pub(super) fn is_copy_shortcut(&self, key: KeyEvent) -> bool {
        key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::SUPER)
            && self.selection.is_some()
    }

    pub(super) fn is_paste_shortcut(&self, key: KeyEvent) -> bool {
        key.code == KeyCode::Char('v')
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER))
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn read_clipboard_command(program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }

    pub(super) fn read_clipboard_text(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            return Self::read_clipboard_command("pbpaste", &[]);
        }
        #[cfg(target_os = "linux")]
        {
            return Self::read_clipboard_command("wl-paste", &["--no-newline"])
                .or_else(|| {
                    Self::read_clipboard_command("xclip", &["-selection", "clipboard", "-o"])
                })
                .or_else(|| Self::read_clipboard_command("xsel", &["--clipboard", "--output"]));
        }
        #[allow(unreachable_code)]
        None
    }

    pub(super) fn paste_from_clipboard(&mut self) -> bool {
        let Some(text) = self.read_clipboard_text() else {
            return false;
        };

        self.handle_paste(text);
        true
    }

    pub(super) fn handle_paste(&mut self, text: String) {
        let text = text.replace('\r', "");
        match self.mode {
            UiMode::Welcome(ref mut state) => {
                match state.current_step() {
                    WelcomeStep::ProviderAuth => state.paste_key(&text),
                    WelcomeStep::WebSearch => state.paste_web_key(&text),
                    _ => {}
                }
                self.needs_redraw = true;
            }
            _ => {
                self.editor.insert_paste(&text);
                if self.ask_state.is_some() {
                    self.sync_ask_from_editor();
                }
                self.needs_redraw = true;
            }
        }
    }

    pub(super) fn extend_selection_lines(&mut self, delta: isize) -> bool {
        let Some(mut selection) = self.selection.clone() else {
            return false;
        };
        let Some(surface) = self.selection_surface(selection.pane) else {
            return false;
        };

        selection.focus = surface.move_pos(selection.focus, delta, 0);
        match selection.pane {
            SelectablePane::Chat => {
                if selection.focus.line < surface.top_line {
                    self.scroll_chat_up(surface.top_line - selection.focus.line);
                } else {
                    let bottom = surface.top_line + surface.rect.height.saturating_sub(1) as usize;
                    if selection.focus.line > bottom {
                        self.scroll_chat_down(selection.focus.line - bottom);
                    }
                }
            }
            SelectablePane::SidebarDetail => {
                if selection.focus.line < surface.top_line {
                    self.sidebar
                        .scroll_detail_up(surface.top_line - selection.focus.line);
                } else {
                    let bottom = surface.top_line + surface.rect.height.saturating_sub(1) as usize;
                    if selection.focus.line > bottom {
                        self.sidebar
                            .scroll_detail_down(selection.focus.line - bottom);
                    }
                }
            }
        }

        self.selection = Some(selection);
        true
    }

    pub(super) fn set_drag_autoscroll(
        &mut self,
        pane: SelectablePane,
        surface: &TextSurface,
        col: u16,
        row: u16,
    ) {
        let top_margin = surface.rect.y.saturating_add(1);
        let bottom_margin = surface
            .rect
            .y
            .saturating_add(surface.rect.height.saturating_sub(2));

        let next = if row <= top_margin {
            let speed = if row <= surface.rect.y { 3 } else { 1 };
            Some(DragAutoScroll {
                pane,
                direction: ScrollDirection::Up,
                speed,
                column: col,
                row,
            })
        } else if row >= bottom_margin {
            let lower_edge = surface.rect.y + surface.rect.height.saturating_sub(1);
            let speed = if row >= lower_edge { 3 } else { 1 };
            Some(DragAutoScroll {
                pane,
                direction: ScrollDirection::Down,
                speed,
                column: col,
                row,
            })
        } else {
            None
        };

        self.drag_autoscroll = next;
    }

    pub(super) fn maybe_autoscroll_selection(&mut self) {
        let Some(auto) = self.drag_autoscroll else {
            return;
        };
        if self.drag_selection != Some(auto.pane) {
            self.drag_autoscroll = None;
            return;
        }

        let Some(surface) = self.selection_surface(auto.pane).cloned() else {
            self.drag_autoscroll = None;
            return;
        };

        let changed = match (auto.pane, auto.direction) {
            (SelectablePane::Chat, ScrollDirection::Up) => {
                let before = self.scroll_offset;
                self.scroll_chat_up(auto.speed);
                self.scroll_offset != before
            }
            (SelectablePane::Chat, ScrollDirection::Down) => {
                let before = self.scroll_offset;
                self.scroll_chat_down(auto.speed);
                self.scroll_offset != before
            }
            (SelectablePane::SidebarDetail, ScrollDirection::Up) => {
                let before = self.sidebar.detail_scroll;
                self.sidebar.scroll_detail_up(auto.speed);
                self.sidebar.detail_scroll != before
            }
            (SelectablePane::SidebarDetail, ScrollDirection::Down) => {
                let before = self.sidebar.detail_scroll;
                self.sidebar.scroll_detail_down(auto.speed);
                self.sidebar.detail_scroll != before
            }
        };

        if !changed {
            return;
        }

        if let Some(selection) = self.selection.as_mut() {
            if selection.pane == auto.pane {
                selection.focus = surface.pos_from_screen_clamped(auto.column, auto.row);
                self.needs_redraw = true;
            }
        }
    }

    #[cfg(feature = "mana-ui")]
    pub(super) fn handle_mana_navigator_mouse(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
    ) -> bool {
        let UiMode::ManaNavigator(ref mut state) = self.mode else {
            return false;
        };

        let terminal_area = Rect {
            x: 0,
            y: 0,
            width: crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80),
            height: crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24),
        };
        let mana_area = super::centered_rect(88, 86, terminal_area);
        if !point_in_rect(mouse.column, mouse.row, Some(mana_area)) {
            return true;
        }

        let inner = Rect {
            x: mana_area.x.saturating_add(1),
            y: mana_area.y.saturating_add(1),
            width: mana_area.width.saturating_sub(2),
            height: mana_area.height.saturating_sub(2),
        };
        if inner.height == 0 || inner.width == 0 {
            return true;
        }
        let content = Rect {
            x: inner.x,
            y: inner.y.saturating_add(1),
            width: inner.width,
            height: inner.height.saturating_sub(1),
        };
        let split_x = if content.width >= 90 {
            content.x + (content.width * 52 / 100)
        } else {
            content.x + content.width
        };
        let in_detail = content.width >= 90 && mouse.column >= split_x;
        let in_tree = mouse.column < split_x;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if in_detail {
                    state.scroll_detail_up_by(self.config.ui.mouse_scroll_lines);
                } else {
                    state.move_up_by(self.config.ui.mouse_scroll_lines);
                }
            }
            MouseEventKind::ScrollDown => {
                if in_detail {
                    state.scroll_detail_down_by(self.config.ui.mouse_scroll_lines);
                } else {
                    state.move_down_by(self.config.ui.mouse_scroll_lines);
                }
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) if in_tree => {
                let row = mouse.row.saturating_sub(content.y) as usize;
                state.select_visible_row(row, content.height as usize);
            }
            _ => {}
        }
        true
    }

    pub(super) fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        self.needs_redraw = true;

        #[cfg(feature = "mana-ui")]
        if self.handle_mana_navigator_mouse(&mouse) {
            return;
        }

        // Session picker intercepts scroll events
        if matches!(self.mode, UiMode::SessionPicker(_)) {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let UiMode::SessionPicker(ref mut state) = self.mode {
                        state.move_up();
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let UiMode::SessionPicker(ref mut state) = self.mode {
                        state.move_down();
                    }
                    self.maybe_load_more_sessions();
                }
                _ => {}
            }
            return;
        }

        let col = mouse.column;
        let row = mouse.row;

        let is_stream = self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Stream;
        let is_inspector =
            self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Inspector;
        let in_list = point_in_rect(col, row, self.sidebar_list_rect);
        let in_detail = point_in_rect(col, row, self.sidebar_detail_rect);
        let in_sidebar = in_list || in_detail;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if in_list && !is_inspector {
                    self.active_pane = Pane::SidebarList;
                    self.sidebar
                        .scroll_list_up(self.sidebar_mouse_scroll_lines());
                } else if in_detail || (in_sidebar && (is_stream || is_inspector)) {
                    self.active_pane = Pane::SidebarDetail;
                    self.sidebar_auto_follow = false;
                    self.sidebar
                        .scroll_detail_up(self.sidebar_mouse_scroll_lines());
                } else {
                    self.active_pane = Pane::Chat;
                    self.scroll_chat_up(self.config.ui.mouse_scroll_lines);
                }
            }
            MouseEventKind::ScrollDown => {
                if in_list && !is_inspector {
                    self.active_pane = Pane::SidebarList;
                    self.sidebar
                        .scroll_list_down(self.sidebar_mouse_scroll_lines());
                } else if in_detail || (in_sidebar && (is_stream || is_inspector)) {
                    self.active_pane = Pane::SidebarDetail;
                    self.sidebar_auto_follow = false;
                    self.sidebar
                        .scroll_detail_down(self.sidebar_mouse_scroll_lines());
                } else {
                    self.active_pane = Pane::Chat;
                    self.scroll_chat_down(self.config.ui.mouse_scroll_lines);
                }
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                if in_list && !is_inspector {
                    self.clear_selection();
                    self.active_pane = Pane::SidebarList;
                    if let Some(lr) = self.sidebar_list_rect {
                        let clicked_row = (row - lr.y) as usize;
                        let clicked_idx = self.sidebar.list_scroll + clicked_row;
                        let total = self.total_tool_calls();
                        if clicked_idx < total {
                            self.focus_tool(clicked_idx);
                        }
                    }
                    return;
                }

                if in_detail || (in_sidebar && (is_stream || is_inspector)) {
                    self.active_pane = Pane::SidebarDetail;
                    if let Some(surface) = self.sidebar_detail_surface.as_ref().cloned() {
                        if !surface.is_empty() {
                            let pos = surface.pos_from_screen_clamped(col, row);
                            self.selection =
                                Some(SelectionState::new(SelectablePane::SidebarDetail, pos, pos));
                            self.drag_selection = Some(SelectablePane::SidebarDetail);
                            self.set_drag_autoscroll(
                                SelectablePane::SidebarDetail,
                                &surface,
                                col,
                                row,
                            );
                        }
                    }
                    return;
                }

                self.active_pane = Pane::Chat;
                if self.select_startup_workflow_at(col, row)
                    || self.select_startup_skill_at(col, row)
                {
                    self.clear_selection();
                    return;
                }

                if let Some(chat_area) = self.chat_surface.as_ref().map(|surface| surface.rect) {
                    if let Some(tool_id) = self.tool_id_at_chat_row(row, chat_area) {
                        self.clear_selection();
                        if let Some(index) = self.find_tool_call_index(&tool_id) {
                            self.focus_tool_in_inspector(index);
                        }
                        return;
                    }
                }

                if let Some(surface) = self.chat_surface.as_ref().cloned() {
                    if !surface.is_empty() {
                        let pos = surface.pos_from_screen_clamped(col, row);
                        self.selection = Some(SelectionState::new(SelectablePane::Chat, pos, pos));
                        self.drag_selection = Some(SelectablePane::Chat);
                        self.set_drag_autoscroll(SelectablePane::Chat, &surface, col, row);
                    }
                }
            }
            MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                let Some(pane) = self.drag_selection else {
                    return;
                };
                let Some(surface) = self.selection_surface(pane).cloned() else {
                    return;
                };
                let pos = surface.pos_from_screen_clamped(col, row);
                if let Some(selection) = self.selection.as_mut() {
                    if selection.pane == pane {
                        selection.focus = pos;
                    }
                }
                self.set_drag_autoscroll(pane, &surface, col, row);
                match pane {
                    SelectablePane::Chat => {
                        self.active_pane = Pane::Chat;
                    }
                    SelectablePane::SidebarDetail => {
                        self.active_pane = Pane::SidebarDetail;
                    }
                }
            }
            MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                self.drag_selection = None;
                self.drag_autoscroll = None;
            }
            _ => {}
        }
    }
}
