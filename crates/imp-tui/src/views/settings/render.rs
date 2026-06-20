use imp_core::config::{
    AgentMode, ContinuePolicy, ShellBackend, WorkflowScopePreference, WriteOverwritePolicy,
};
use imp_core::tools::web::types::SearchProvider;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use crate::theme::Theme;

use super::layout::{scrolled_screen_y, settings_scroll_offset, total_settings_rows};
use super::options::{animation_label, thinking_label};
use super::rows::{render_field, render_save_row, render_settings_header, render_settings_tabs};
use super::{field_index, SettingsField, SettingsState};

pub struct SettingsView<'a> {
    state: &'a SettingsState,
    theme: &'a Theme,
}

impl<'a> SettingsView<'a> {
    pub fn new(state: &'a SettingsState, theme: &'a Theme) -> Self {
        Self { state, theme }
    }
}

impl Widget for SettingsView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 10 || area.width < 30 {
            return;
        }

        Clear.render(area, buf);

        let title = if self.state.dirty {
            " Settings * "
        } else {
            " Settings "
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(self.theme.accent_style());
        let inner = block.inner(area);
        block.render(area, buf);

        let total_rows = total_settings_rows(self.state);
        let scroll_offset = settings_scroll_offset(self.state, inner.height);

        let mut row: u16 = 0;

        render_settings_header(self.state, self.theme, buf, inner, scroll_offset, &mut row);
        render_settings_tabs(self.state, self.theme, buf, inner, scroll_offset, &mut row);

        if self.state.visible_fields().is_empty() {
            if let Some(message) = self.state.tab.empty_message() {
                if let Some(y) = scrolled_screen_y(inner, row, scroll_offset) {
                    let line = Line::from(vec![
                        Span::raw("  "),
                        Span::styled(message, self.theme.muted_style()),
                    ]);
                    buf.set_line(inner.x, y, &line, inner.width);
                }
                row += 1;
            }
        } else {
            for field in self.state.visible_fields() {
                render_settings_field(
                    self.state,
                    self.theme,
                    buf,
                    inner,
                    scroll_offset,
                    &mut row,
                    *field,
                );
            }
        }

        row += 1;
        render_save_row(self.state, self.theme, buf, inner, scroll_offset, row);

        if scroll_offset > 0 {
            let hint = Line::from(Span::styled("↑ more", self.theme.muted_style()));
            buf.set_line(inner.x + inner.width.saturating_sub(7), inner.y, &hint, 7);
        }
        if scroll_offset + inner.height < total_rows {
            let hint = Line::from(Span::styled("↓ more", self.theme.muted_style()));
            let y = inner.y + inner.height.saturating_sub(1);
            buf.set_line(inner.x + inner.width.saturating_sub(7), y, &hint, 7);
        }
    }
}

fn render_settings_field(
    state: &SettingsState,
    theme: &Theme,
    buf: &mut Buffer,
    inner: Rect,
    scroll_offset: u16,
    row: &mut u16,
    field: SettingsField,
) {
    match field {
        SettingsField::Model => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(SettingsField::Model),
            "Model",
            &state.model,
            "← →",
        ),
        SettingsField::ChosenModels => {
            let chosen_hint = if state.model_is_chosen(&state.model) {
                "← → toggle current"
            } else {
                "← → add current"
            };
            let chosen_summary = state.chosen_models_summary();
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(SettingsField::ChosenModels),
                "Chosen models",
                &chosen_summary,
                chosen_hint,
            );
        }
        SettingsField::Theme => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(SettingsField::Theme),
            "Color theme",
            &state.theme_name,
            "← → (UI colors)",
        ),
        SettingsField::ThinkingLevel => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(SettingsField::ThinkingLevel),
            "Thinking level",
            thinking_label(state.thinking_level),
            "← →",
        ),
        SettingsField::MaxTokens => {
            let value = if state.editing_number && state.current_field() == SettingsField::MaxTokens
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.max_tokens.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Max tokens",
                &value,
                "← → / type",
            );
        }
        SettingsField::MaxTurns => {
            let value = if state.editing_number && state.current_field() == SettingsField::MaxTurns
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.max_turns.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Max turns",
                &value,
                "← → / type",
            );
        }
        SettingsField::ObservationMask => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::ObservationMask
            {
                format!("{}▎", state.edit_buffer)
            } else {
                format!("{:.0}%", state.observation_mask * 100.0)
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Observation mask",
                &value,
                "← →",
            );
        }
        SettingsField::ReadMaxLines => {
            let value =
                if state.editing_number && state.current_field() == SettingsField::ReadMaxLines {
                    format!("{}▎", state.edit_buffer)
                } else {
                    state.read_max_lines.to_string()
                };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Read max lines",
                &value,
                "← → / type (0 = no limit)",
            );
        }
        SettingsField::SidebarWidth => {
            let value =
                if state.editing_number && state.current_field() == SettingsField::SidebarWidth {
                    format!("{}▎", state.edit_buffer)
                } else {
                    format!("{}%", state.sidebar_width)
                };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Inspector width",
                &value,
                "← → / type",
            );
        }
        SettingsField::WordWrap => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Word wrap",
            if state.word_wrap { "on" } else { "off" },
            "← →",
        ),
        SettingsField::Animations => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Animations",
            animation_label(state.animations),
            "← →",
        ),
        SettingsField::AutoOpenSidebar => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Auto-open sidebar",
            if state.auto_open_sidebar { "on" } else { "off" },
            "← →",
        ),
        SettingsField::SidebarAutoOpenWidth => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::SidebarAutoOpenWidth
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.sidebar_auto_open_width.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Auto-open width",
                &value,
                "← → / type",
            );
        }
        SettingsField::ThinkingLines => {
            let value =
                if state.editing_number && state.current_field() == SettingsField::ThinkingLines {
                    format!("{}▎", state.edit_buffer)
                } else {
                    state.thinking_lines.to_string()
                };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Thinking lines",
                &value,
                "← → / type",
            );
        }
        SettingsField::StreamingLines => {
            let value =
                if state.editing_number && state.current_field() == SettingsField::StreamingLines {
                    format!("{}▎", state.edit_buffer)
                } else {
                    state.streaming_lines.to_string()
                };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Streaming lines",
                &value,
                "← → / type",
            );
        }
        SettingsField::MouseScrollLines => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::MouseScrollLines
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.mouse_scroll_lines.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Mouse scroll",
                &value,
                "← → / type",
            );
        }
        SettingsField::KeyboardScrollLines => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::KeyboardScrollLines
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.keyboard_scroll_lines.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Keyboard scroll",
                &value,
                "← → / type",
            );
        }
        SettingsField::ShowTimestamps => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Show timestamps",
            if state.show_timestamps { "on" } else { "off" },
            "← →",
        ),
        SettingsField::ShowCost => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Show cost",
            if state.show_cost { "on" } else { "off" },
            "← →",
        ),
        SettingsField::ShowContextUsage => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Show context",
            if state.show_context_usage {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::NotifyOnAgentComplete => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Bell on done",
            if state.notify_on_agent_complete {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::ContinuePolicy => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Looping",
            match state.continue_policy {
                ContinuePolicy::Disabled => "off",
                ContinuePolicy::Conservative => "conservative",
                ContinuePolicy::Balanced => "balanced",
                ContinuePolicy::Aggressive => "aggressive",
            },
            "← →",
        ),
        SettingsField::AgentMode => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Agent mode",
            match state.agent_mode {
                AgentMode::Full => "full",
                AgentMode::Worker => "worker",
                AgentMode::Orchestrator => "orchestrator",
                AgentMode::Planner => "planner",
                AgentMode::Reviewer => "reviewer",
                AgentMode::Auditor => "auditor",
            },
            "← →",
        ),
        SettingsField::WriteOverwritePolicy => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Write safety",
            match state.write_overwrite_policy {
                WriteOverwritePolicy::Warn => "warn",
                WriteOverwritePolicy::RequireRead => "require read",
                WriteOverwritePolicy::BlockStale => "block stale",
                WriteOverwritePolicy::Deny => "deny overwrites",
            },
            "← →",
        ),
        SettingsField::ShellBackend => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Shell backend",
            match state.shell_backend {
                ShellBackend::Sh => "sh",
                ShellBackend::Rush => "rush",
                ShellBackend::RushDaemon => "rush-daemon",
            },
            "← →",
        ),
        SettingsField::LuaNativeTools => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Lua native tools",
            if state.lua_native_tools { "on" } else { "off" },
            "← →",
        ),
        SettingsField::LuaShellExec => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Lua shell exec",
            if state.lua_shell_exec { "on" } else { "off" },
            "← →",
        ),
        SettingsField::LuaHttp => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Lua HTTP",
            if state.lua_http { "on" } else { "off" },
            "← →",
        ),
        SettingsField::LuaSecrets => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Lua secrets",
            if state.lua_secrets { "on" } else { "off" },
            "← →",
        ),
        SettingsField::ImproveAutoTurnBudget => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::ImproveAutoTurnBudget
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.improve_auto_turn_budget.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Improve turns",
                &value,
                "← → / type",
            );
        }
        SettingsField::LoopTurnBudget => {
            let value =
                if state.editing_number && state.current_field() == SettingsField::LoopTurnBudget {
                    format!("{}▎", state.edit_buffer)
                } else if state.loop_turn_budget == 0 {
                    "unlimited".to_string()
                } else {
                    state.loop_turn_budget.to_string()
                };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Loop turns",
                &value,
                "← → / type",
            );
        }
        SettingsField::WebSearchProvider => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Web provider",
            match state.web_search_provider {
                None => "auto",
                Some(SearchProvider::Tavily) => "tavily",
                Some(SearchProvider::Exa) => "exa",
                Some(SearchProvider::Linkup) => "linkup",
                Some(SearchProvider::Perplexity) => "perplexity",
                Some(SearchProvider::GitHub) => "github",
            },
            "← →",
        ),
        SettingsField::WorkflowScope => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Default scope",
            match state.workflow_scope {
                WorkflowScopePreference::Project => "project",
                WorkflowScopePreference::Root => "root",
            },
            "← →",
        ),
        SettingsField::WorkflowAutoCommit => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Commit on close",
            if state.workflow_auto_commit {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::WorkflowAutoCloseParent => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Auto-close parent",
            if state.workflow_auto_close_parent {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::WorkflowVerifyTimeout => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::WorkflowVerifyTimeout
            {
                format!("{}▎", state.edit_buffer)
            } else if state.workflow_verify_timeout == 0 {
                "default".to_string()
            } else {
                format!("{}s", state.workflow_verify_timeout)
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Verify timeout",
                &value,
                "← → / type (0 = default)",
            );
        }
        SettingsField::WorkflowRunBackground => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Run in background",
            if state.workflow_run_background {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::WorkflowMaxWorkers => {
            let value = if state.editing_number
                && state.current_field() == SettingsField::WorkflowMaxWorkers
            {
                format!("{}▎", state.edit_buffer)
            } else {
                state.workflow_max_workers.to_string()
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Max workers",
                &value,
                "← → / type",
            );
        }
        SettingsField::WorkflowReviewAfterRun => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Review after run",
            if state.workflow_review_after_run {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::WorkflowContinueAfterFailure => render_field(
            state,
            theme,
            buf,
            inner,
            scroll_offset,
            row,
            field_index(field),
            "Continue after failure",
            if state.workflow_continue_after_failure {
                "on"
            } else {
                "off"
            },
            "← →",
        ),
        SettingsField::TavilyApiKey => {
            let value = if state.tavily_api_key.is_empty() {
                if state.tavily_configured {
                    "configured (press Enter to replace)".to_string()
                } else {
                    "not set".to_string()
                }
            } else {
                format!(
                    "{}▎",
                    "•".repeat(state.tavily_api_key.chars().count().max(1))
                )
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Tavily API key",
                &value,
                "Enter to edit",
            );
        }
        SettingsField::ExaApiKey => {
            let value = if state.exa_api_key.is_empty() {
                if state.exa_configured {
                    "configured (press Enter to replace)".to_string()
                } else {
                    "not set".to_string()
                }
            } else {
                format!("{}▎", "•".repeat(state.exa_api_key.chars().count().max(1)))
            };
            render_field(
                state,
                theme,
                buf,
                inner,
                scroll_offset,
                row,
                field_index(field),
                "Exa API key",
                &value,
                "Enter to edit",
            );
        }
        SettingsField::Save => {}
    }
}
