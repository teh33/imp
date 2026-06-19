use ratatui::layout::Rect;

use super::{SettingsField, SettingsState};

pub(super) enum SettingsRow {
    Header,
    Tabs,
    Field(SettingsField),
    Save,
    EmptyMessage,
}

pub(super) fn visit_settings_rows(state: &SettingsState, mut visit: impl FnMut(SettingsRow, u16)) {
    let mut row = 0;
    visit(SettingsRow::Header, row);
    row += 1;
    visit(SettingsRow::Tabs, row);
    row += 2;

    let fields = state.visible_fields();
    if fields.is_empty() {
        visit(SettingsRow::EmptyMessage, row);
        row += 1;
    } else {
        for field in fields {
            visit(SettingsRow::Field(*field), row);
            row += 1;
        }
    }

    row += 1;
    visit(SettingsRow::Save, row);
}

pub(crate) fn total_settings_rows(state: &SettingsState) -> u16 {
    let mut total = 0;
    visit_settings_rows(state, |_, row| {
        total = total.max(row.saturating_add(1));
    });
    total
}

pub(crate) fn selected_settings_row(state: &SettingsState) -> u16 {
    let selected = state.current_field();
    let mut selected_row = 0;
    visit_settings_rows(state, |entry, row| match entry {
        SettingsRow::Field(field) if field == selected => selected_row = row,
        SettingsRow::Save if selected == SettingsField::Save => selected_row = row,
        _ => {}
    });
    selected_row
}

pub(crate) fn settings_scroll_offset(state: &SettingsState, visible_rows: u16) -> u16 {
    if visible_rows == 0 {
        return 0;
    }

    let total_rows = total_settings_rows(state);
    let max_offset = total_rows.saturating_sub(visible_rows);
    let selected_row = selected_settings_row(state);
    let bottom = visible_rows.saturating_sub(1);
    selected_row.saturating_sub(bottom).min(max_offset)
}

pub(super) fn scrolled_screen_y(inner: Rect, logical_row: u16, scroll_offset: u16) -> Option<u16> {
    let visible_row = logical_row.checked_sub(scroll_offset)?;
    if visible_row >= inner.height {
        return None;
    }
    Some(inner.y + visible_row)
}
