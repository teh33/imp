use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use imp_llm::ThinkingLevel;

use crate::app::WelcomeAuthMethod;

use super::{WelcomeStep, WelcomeView, STEPS};

impl Widget for WelcomeView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 10 || area.width < 30 {
            return;
        }

        Clear.render(area, buf);

        let step_indicator = format!(
            " Welcome ({}/{}) ",
            self.state.normalized_step() + 1,
            STEPS.len()
        );
        let block = Block::default()
            .title(step_indicator)
            .borders(Borders::ALL)
            .border_style(self.theme.accent_style());
        let inner = block.inner(area);
        block.render(area, buf);

        match self.state.current_step() {
            WelcomeStep::Welcome => self.render_welcome(inner, buf),
            WelcomeStep::ProviderAuth => self.render_provider_auth(inner, buf, false),
            WelcomeStep::ModelThinking => self.render_model_thinking(inner, buf),
            WelcomeStep::WebSearch => self.render_web_search(inner, buf),
            WelcomeStep::Done => self.render_done(inner, buf),
        }
    }
}

impl WelcomeView<'_> {
    fn render_welcome(&self, area: Rect, buf: &mut Buffer) {
        self.render_provider_auth(area, buf, true);
    }

    fn render_provider_auth(&self, area: Rect, buf: &mut Buffer, include_intro: bool) {
        let mut row: u16 = 0;
        let x = area.x;

        let title_text = if include_intro {
            "  Welcome to imp — choose how to sign in"
        } else {
            "  Choose your AI provider"
        };
        let title = Line::from(Span::styled(
            title_text,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 2;

        for (i, status) in self.state.providers.iter().enumerate() {
            if row >= area.height.saturating_sub(4) {
                break;
            }
            let is_selected = i == self.state.provider_selected;
            let marker = if is_selected { "▸ " } else { "  " };

            let auth_hint = if status.env_detected {
                let detected_var = status
                    .meta
                    .env_vars
                    .iter()
                    .find(|v| std::env::var(v).is_ok())
                    .copied()
                    .unwrap_or(status.meta.env_vars.first().copied().unwrap_or(""));
                format!("  ({} detected ✓)", detected_var)
            } else if status.stored {
                "  (saved ✓)".to_string()
            } else {
                String::new()
            };

            let label_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let line = Line::from(vec![
                Span::styled(format!("  {marker}"), self.theme.accent_style()),
                Span::styled(status.meta.name, label_style),
                Span::styled(auth_hint, self.theme.success_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let Some(selected) = self.state.selected_provider() else {
            let line = Line::from(Span::styled(
                "  No providers available",
                self.theme.muted_style(),
            ));
            buf.set_line(x, area.y + row, &line, area.width);
            return;
        };
        if selected.has_auth() {
            let ready = Line::from(vec![
                Span::styled("  ✓ ", self.theme.success_style()),
                Span::styled("Ready to connect.", self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        } else {
            let oauth_supported = self.state.selected_provider_supports_oauth();
            let api_key_supported = self.state.selected_provider_supports_api_key_setup();
            if oauth_supported && api_key_supported {
                let oauth_style = if self.state.auth_method() == WelcomeAuthMethod::OAuth {
                    self.theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.muted_style()
                };
                let api_style = if self.state.auth_method() == WelcomeAuthMethod::ApiKey {
                    self.theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    self.theme.muted_style()
                };
                let tabs = Line::from(vec![
                    Span::styled("  Auth: ", self.theme.muted_style()),
                    Span::styled("[ OAuth ]", oauth_style),
                    Span::raw("  "),
                    Span::styled("[ API key ]", api_style),
                    Span::styled("  Tab/←→ to switch", self.theme.muted_style()),
                ]);
                buf.set_line(x, area.y + row, &tabs, area.width);
                row += 2;
            }

            match self.state.auth_method() {
                WelcomeAuthMethod::OAuth if oauth_supported => {
                    if selected.meta.id == "anthropic" {
                        let warning = Line::from(Span::styled(
                            "  Anthropic OAuth is not supported. Proceed with caution.",
                            self.theme.error_style(),
                        ));
                        buf.set_line(x, area.y + row, &warning, area.width);
                        row += 1;
                    }

                    let oauth = if self.state.oauth_pending {
                        "  OAuth: waiting for browser login..."
                    } else {
                        "  OAuth: press Enter to sign in in your browser"
                    };
                    buf.set_line(
                        x,
                        area.y + row,
                        &Line::from(Span::styled(oauth, self.theme.accent_style())),
                        area.width,
                    );
                    row += 1;

                    if let Some(status) = self.state.oauth_status.as_deref() {
                        let status_line = Line::from(Span::styled(
                            format!("  {status}"),
                            self.theme.muted_style(),
                        ));
                        buf.set_line(x, area.y + row, &status_line, area.width);
                        row += 1;
                    }
                    if let Some(url) = self.state.oauth_url.as_deref() {
                        let url_line = Line::from(vec![
                            Span::styled("  URL: ", self.theme.muted_style()),
                            Span::styled(url, Style::default().fg(self.theme.accent)),
                        ]);
                        buf.set_line(x, area.y + row, &url_line, area.width);
                        row += 1;
                    }
                }
                _ if api_key_supported => {
                    let prompt_line =
                        Line::from(vec![Span::styled("  API Key: ", self.theme.muted_style())]);
                    buf.set_line(x, area.y + row, &prompt_line, area.width);
                    row += 1;

                    let display_key = if self.state.key_input.is_empty() {
                        "  ┌─ paste your key here ─────────────────┐".to_string()
                    } else {
                        let masked: String = self
                            .state
                            .key_input
                            .chars()
                            .enumerate()
                            .map(|(i, c)| if i < 6 { c } else { '•' })
                            .collect();
                        format!(
                            "  ┌ {masked}▎{} ┐",
                            " ".repeat(40usize.saturating_sub(masked.len() + 1))
                        )
                    };
                    let key_style = if self.state.key_input.is_empty() {
                        self.theme.muted_style()
                    } else {
                        Style::default()
                    };
                    let key_line = Line::from(Span::styled(display_key, key_style));
                    buf.set_line(x, area.y + row, &key_line, area.width);
                    row += 1;

                    let url_line = Line::from(vec![
                        Span::styled("  Get a key: ", self.theme.muted_style()),
                        Span::styled(
                            selected.meta.docs_url,
                            Style::default().fg(self.theme.accent),
                        ),
                    ]);
                    buf.set_line(x, area.y + row, &url_line, area.width);
                    row += 1;
                }
                _ => {}
            }

            if let Some(ref error) = self.state.key_error {
                row += 1;
                let error_line =
                    Line::from(Span::styled(format!("  {error}"), self.theme.error_style()));
                buf.set_line(x, area.y + row, &error_line, area.width);
            }
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::styled("    ", Style::default()),
                Span::styled("O ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("OAuth login", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Select provider", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_model_thinking(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let title = Line::from(Span::styled(
            "  Default model & thinking level",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 2;

        let subtitle = Line::from(Span::styled("  Model:", self.theme.muted_style()));
        buf.set_line(x, area.y + row, &subtitle, area.width);
        row += 1;

        let visible_models = 6usize;
        let selected_model = self.state.normalized_model_selected();
        let start = selected_model.saturating_sub(visible_models / 2);
        let end = (start + visible_models).min(self.state.models.len());
        let start = end.saturating_sub(visible_models);

        for model_i in start..end {
            if row >= area.height.saturating_sub(6) {
                break;
            }
            let model = &self.state.models[model_i];
            let is_selected = model_i == selected_model;
            let marker = if is_selected { "▸ " } else { "  " };

            let name_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let context_str = format!("{}k", model.context_window / 1000);
            let price_str = format!(
                "${:.2}/{:.2}",
                model.pricing.input_per_mtok, model.pricing.output_per_mtok
            );

            let line = Line::from(vec![
                Span::styled(format!("    {marker}"), self.theme.accent_style()),
                Span::styled(format!("{:<36}", &model.name), name_style),
                Span::styled(format!("{context_str:>5}"), self.theme.muted_style()),
                Span::raw("  "),
                Span::styled(price_str, self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let thinking_label = match self.state.thinking_level {
            ThinkingLevel::Off => "Off",
            ThinkingLevel::Minimal => "Minimal",
            ThinkingLevel::Low => "Low",
            ThinkingLevel::Medium => "Medium",
            ThinkingLevel::High => "High",
            ThinkingLevel::XHigh => "XHigh",
        };
        let thinking_line = Line::from(vec![
            Span::styled("  Thinking:  ", self.theme.muted_style()),
            Span::styled("← ", self.theme.accent_style()),
            Span::styled(
                thinking_label,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" →", self.theme.accent_style()),
        ]);
        buf.set_line(x, area.y + row, &thinking_line, area.width);
        row += 2;

        let hint = Line::from(Span::styled(
            "  You can change these anytime with Ctrl+L and Shift+Tab.",
            self.theme.muted_style(),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &hint, area.width);
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Model", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("←→ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Thinking", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_web_search(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let title = Line::from(Span::styled(
            "  Optional web search setup",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &title, area.width);
        row += 1;

        let subtitle = Line::from(Span::styled(
            "  Add Tavily or Exa now so the web tool can search immediately.",
            self.theme.muted_style(),
        ));
        buf.set_line(x, area.y + row, &subtitle, area.width);
        row += 2;

        for (i, provider) in self.state.web_providers.iter().enumerate() {
            if row >= area.height.saturating_sub(6) {
                break;
            }
            let is_selected = i == self.state.web_provider_selected;
            let marker = if is_selected { "▸ " } else { "  " };
            let mut status = String::new();
            if provider.id == "none" {
                status = "  (skip)".to_string();
            } else if provider.env_detected {
                status = format!("  ({} detected ✓)", provider.env_key);
            } else if provider.stored {
                status = "  (saved ✓)".to_string();
            }
            let label_style = if is_selected {
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let line = Line::from(vec![
                Span::styled(format!("  {marker}"), self.theme.accent_style()),
                Span::styled(provider.label, label_style),
                Span::styled(status, self.theme.success_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;
        let Some(selected) = self.state.selected_web_provider() else {
            let line = Line::from(Span::styled(
                "  No web search providers available",
                self.theme.muted_style(),
            ));
            buf.set_line(x, area.y + row, &line, area.width);
            return;
        };
        if selected.id != "none" && !selected.has_auth() {
            let prompt_line =
                Line::from(vec![Span::styled("  API Key: ", self.theme.muted_style())]);
            buf.set_line(x, area.y + row, &prompt_line, area.width);
            row += 1;

            let display_key = if self.state.web_key_input.is_empty() {
                "  ┌─ paste your key here ─────────────────┐".to_string()
            } else {
                let masked: String = self
                    .state
                    .web_key_input
                    .chars()
                    .enumerate()
                    .map(|(i, c)| if i < 6 { c } else { '•' })
                    .collect();
                format!(
                    "  ┌ {masked}▎{} ┐",
                    " ".repeat(40usize.saturating_sub(masked.len() + 1))
                )
            };
            let key_style = if self.state.web_key_input.is_empty() {
                self.theme.muted_style()
            } else {
                Style::default()
            };
            let key_line = Line::from(Span::styled(display_key, key_style));
            buf.set_line(x, area.y + row, &key_line, area.width);
            row += 1;

            let url_line = Line::from(vec![
                Span::styled("  Get a key: ", self.theme.muted_style()),
                Span::styled(selected.docs_url, Style::default().fg(self.theme.accent)),
            ]);
            buf.set_line(x, area.y + row, &url_line, area.width);
        } else if selected.id == "none" {
            let ready = Line::from(vec![
                Span::styled("  ↷ ", self.theme.muted_style()),
                Span::styled(
                    "Skipping web search setup for now.",
                    self.theme.muted_style(),
                ),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        } else {
            let ready = Line::from(vec![
                Span::styled("  ✓ ", self.theme.success_style()),
                Span::styled("Web search provider is ready.", self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &ready, area.width);
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Continue", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Select provider", self.theme.muted_style()),
                Span::raw("    "),
                Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Back", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }

    fn render_done(&self, area: Rect, buf: &mut Buffer) {
        let mut row: u16 = 0;
        let x = area.x;

        let header = Line::from(Span::styled(
            "  ✓ You're all set.",
            Style::default()
                .fg(self.theme.success)
                .add_modifier(Modifier::BOLD),
        ));
        buf.set_line(x, area.y + row, &header, area.width);
        row += 2;

        let provider_name = self
            .state
            .selected_provider()
            .map(|provider| provider.meta.name)
            .unwrap_or("not configured");
        let web_provider_name = self
            .state
            .resolved_web_provider
            .as_deref()
            .filter(|id| *id != "none")
            .map(|id| {
                self.state
                    .web_providers
                    .iter()
                    .find(|provider| provider.id == id)
                    .map(|provider| provider.label)
                    .unwrap_or(id)
            })
            .unwrap_or("not configured");
        let model_name = self
            .state
            .selected_model()
            .map(|m| m.name)
            .unwrap_or_else(|| "default".to_string());
        let thinking_label = match self.state.thinking_level {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
        };

        let summary_lines = [
            format!("  Provider:  {provider_name}"),
            format!("  Model:     {model_name}"),
            format!("  Thinking:  {thinking_label}"),
            format!("  Web:       {web_provider_name}"),
        ];

        for line_text in &summary_lines {
            if row >= area.height {
                return;
            }
            let line = Line::from(Span::styled(line_text.as_str(), Style::default()));
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        row += 1;

        let config_hint = Line::from(Span::styled(
            "  Config saved to ~/.config/imp/config.toml",
            self.theme.muted_style(),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &config_hint, area.width);
            row += 1;
        }

        row += 1;

        let tips_header = Line::from(Span::styled(
            "  Quick tips:",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        if row < area.height {
            buf.set_line(x, area.y + row, &tips_header, area.width);
            row += 1;
        }

        let tips = [
            ("Enter", "Send a message"),
            ("Ctrl+C", "Clear / Abort / Quit"),
            ("Ctrl+L", "Switch model"),
            ("Shift+Tab", "Cycle thinking level"),
            ("@file", "Attach file context"),
            ("/command", "Slash commands"),
        ];

        for (key, desc) in &tips {
            if row >= area.height.saturating_sub(2) {
                break;
            }
            let line = Line::from(vec![
                Span::styled(format!("    {key:<12}"), self.theme.accent_style()),
                Span::styled(*desc, self.theme.muted_style()),
            ]);
            buf.set_line(x, area.y + row, &line, area.width);
            row += 1;
        }

        if area.height > 2 {
            let footer_y = area.y + area.height - 1;
            let footer = Line::from(vec![
                Span::styled("  Enter ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("Start using imp", self.theme.muted_style()),
            ]);
            buf.set_line(x, footer_y, &footer, area.width);
        }
    }
}
