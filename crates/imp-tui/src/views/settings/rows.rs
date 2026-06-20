use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

use super::layout::scrolled_screen_y;
use super::{SettingsField, SettingsState, SETTINGS_TABS};

pub(super) fn render_settings_header(
    state: &SettingsState,
    theme: &Theme,
    buf: &mut Buffer,
    inner: Rect,
    scroll_offset: u16,
    row: &mut u16,
) {
    let header = Line::from(Span::styled(
        "  Tab switch  ↑/↓ move  ←/→ change  Enter edit  Esc close",
        theme.muted_style(),
    ));
    if let Some(y) = scrolled_screen_y(inner, *row, scroll_offset) {
        buf.set_line(inner.x, y, &header, inner.width);
    }
    *row += 2;

    let _ = state;
}

pub(super) fn render_settings_tabs(
    state: &SettingsState,
    theme: &Theme,
    buf: &mut Buffer,
    inner: Rect,
    scroll_offset: u16,
    row: &mut u16,
) {
    let mut spans = vec![Span::raw("  ")];
    for (idx, tab) in SETTINGS_TABS.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", theme.muted_style()));
        }
        let label = format!(" {} ", tab.label());
        if *tab == state.tab {
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
        } else {
            spans.push(Span::styled(label, theme.muted_style()));
        }
    }

    if let Some(y) = scrolled_screen_y(inner, *row, scroll_offset) {
        buf.set_line(inner.x, y, &Line::from(spans), inner.width);
    }
    *row += 2;
}

pub(super) fn render_save_row(
    state: &SettingsState,
    theme: &Theme,
    buf: &mut Buffer,
    inner: Rect,
    scroll_offset: u16,
    row: u16,
) {
    let Some(y) = scrolled_screen_y(inner, row, scroll_offset) else {
        return;
    };
    let is_save = state.current_field() == SettingsField::Save;
    let save_style = if is_save {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.muted_style()
    };
    let marker = if is_save { "▸ " } else { "  " };
    let dirty_hint = if state.dirty {
        " (unsaved changes)"
    } else {
        ""
    };
    let line = Line::from(vec![
        Span::styled(marker, theme.accent_style()),
        Span::styled("[ Save to config.toml ]", save_style),
        Span::styled(dirty_hint, theme.warning_style()),
    ]);
    buf.set_line(inner.x, y, &line, inner.width);
}

/// Render one settings field row.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_field(
    state: &SettingsState,
    theme: &Theme,
    buf: &mut Buffer,
    inner: Rect,
    scroll_offset: u16,
    row: &mut u16,
    field_idx: usize,
    label: &str,
    value: &str,
    hint: &str,
) {
    let logical_row = *row;
    let Some(screen_y) = scrolled_screen_y(inner, logical_row, scroll_offset) else {
        *row += 1;
        return;
    };

    let is_selected = field_idx == state.normalized_selected();
    let marker = if is_selected { "▸ " } else { "  " };

    let label_style = if is_selected {
        theme.selected_style()
    } else {
        Style::default()
    };
    let value_style = if is_selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let label_width = 22;
    let line = Line::from(vec![
        Span::styled(marker, theme.accent_style()),
        Span::styled(format!("{label:<label_width$}"), label_style),
        Span::styled(value, value_style),
        Span::raw("  "),
        Span::styled(hint, theme.muted_style()),
    ]);
    buf.set_line(inner.x, screen_y, &line, inner.width);
    *row += 1;
}
