use crossterm::event::KeyEvent;

use crate::keybindings::{self, Action};
use crate::views::model_selector::ModelSelection;

use super::{App, UiMode};

impl App {
    pub(super) fn handle_overlay_key(&mut self, key: KeyEvent) {
        let action = keybindings::resolve_overlay(key);

        match action {
            Some(Action::OverlayDismiss) => {
                // If dismissing command palette, clear the editor's slash prefix
                if matches!(self.mode, UiMode::CommandPalette(_)) {
                    self.editor.clear();
                }
                self.mode = UiMode::Normal;
            }
            Some(Action::OverlayUp) => match &mut self.mode {
                UiMode::ModelSelector(s) => s.move_up(),
                UiMode::CommandPalette(s) => s.move_up(),
                UiMode::LoginPicker(s) => s.move_up(),
                UiMode::SecretsPicker(s) => s.move_up(),
                _ => {}
            },
            Some(Action::OverlayDown) => match &mut self.mode {
                UiMode::ModelSelector(s) => s.move_down(),
                UiMode::CommandPalette(s) => s.move_down(),
                UiMode::LoginPicker(s) => s.move_down(),
                UiMode::SecretsPicker(s) => s.move_down(),
                _ => {}
            },
            Some(Action::OverlayFilter(c)) => match &mut self.mode {
                UiMode::ModelSelector(s) => s.push_filter(c),
                UiMode::CommandPalette(s) => {
                    s.push_filter(c);
                    self.editor.insert_char(c);
                }
                _ => {}
            },
            Some(Action::OverlayBackspace) => match &mut self.mode {
                UiMode::ModelSelector(s) => s.pop_filter(),
                UiMode::CommandPalette(s) => {
                    s.pop_filter();
                    self.editor.delete_back();
                    // If editor is empty (backspaced past /), dismiss
                    if self.editor.is_empty() {
                        self.mode = UiMode::Normal;
                    }
                }
                _ => {}
            },
            Some(Action::OverlayLeft) => {
                if let UiMode::CommandPalette(s) = &mut self.mode {
                    s.prev_page();
                }
            }
            Some(Action::OverlayRight) => {
                if let UiMode::CommandPalette(s) = &mut self.mode {
                    s.next_page();
                }
            }
            Some(Action::OverlaySelect) => {
                self.handle_overlay_select();
            }
            _ => {}
        }
    }

    pub(super) fn handle_overlay_select(&mut self) {
        // Take ownership of mode to process selection
        let old_mode = std::mem::replace(&mut self.mode, UiMode::Normal);
        match old_mode {
            UiMode::ModelSelector(state) => {
                if let Some(selection) = state.selected_choice() {
                    match selection {
                        ModelSelection::Builtin(model) => {
                            self.model_name = model.id.clone();
                            self.context_window = model.context_window;
                        }
                        ModelSelection::Custom(model_id) => {
                            self.model_name = model_id;
                            if let Some(meta) =
                                self.model_registry.resolve_meta(&self.model_name, None)
                            {
                                self.context_window = meta.context_window;
                            }
                        }
                    }
                }
            }
            UiMode::CommandPalette(state) => {
                if let Some(cmd) = state.selected_command() {
                    self.editor.clear();
                    self.execute_command(&cmd.name.clone());
                }
            }
            UiMode::LoginPicker(state) => {
                if let Some(provider) = state.selected_provider() {
                    self.start_login(provider.id);
                }
            }
            UiMode::SecretsPicker(state) => {
                if let Some(provider) = state.selected_provider() {
                    self.start_secrets_flow(&provider.id);
                }
            }
            _ => {
                self.mode = old_mode;
            }
        }
    }
}
