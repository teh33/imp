use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::Frame;

use crate::animation::AnimationState;
use crate::views::chat::{
    build_chat_render_data, build_click_map_from_rendered_lines, build_text_surface_from_lines,
    clamped_scroll_offset_for_total_lines, scroll_offset_for_message_at_top,
};
use crate::views::editor::EditorView;
use crate::views::sidebar::{
    build_detail_render_data, build_stream_lines, SidebarDetailRenderData,
};
use crate::views::startup::{StartupPanelData, StartupSection};
use crate::views::status::StatusInfo;
use crate::views::tools::DisplayToolCall;

use super::*;

impl App {
    // ── Rendering ───────────────────────────────────────────────

    pub(super) fn estimated_active_context_tokens(&self) -> u32 {
        let Some(meta) = self.current_model_meta_for_persistence() else {
            return self
                .session
                .get_active_messages()
                .iter()
                .map(|message| {
                    let json = serde_json::to_string(message).unwrap_or_default();
                    imp_core::context::estimate_tokens(&json)
                })
                .sum();
        };

        self.session
            .get_active_messages()
            .iter()
            .map(|message| imp_core::context::estimate_message_tokens_for_model(message, &meta))
            .sum()
    }

    pub(super) fn display_context_tokens(&self) -> u32 {
        self.current_context_tokens
            .max(self.estimated_active_context_tokens())
    }

    pub(super) fn active_context_window(&self) -> u32 {
        self.current_model_meta_for_persistence()
            .map(|meta| imp_core::context::context_budget_for_meta(&meta).display_window)
            .unwrap_or(self.context_window)
    }

    pub(super) fn current_activity_state(&self) -> AnimationState {
        let active_tools = self
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter(|tc| tc.output.is_none() && !tc.is_error)
            .count() as u32;

        let latest_streaming = self.messages.iter().rev().find(|m| m.is_streaming);
        let has_visible_content = latest_streaming
            .map(|m| !m.content.trim().is_empty())
            .unwrap_or(false);
        let has_tools_in_turn = latest_streaming
            .map(|m| !m.tool_calls.is_empty())
            .unwrap_or(active_tools > 0);

        if self.compaction_task.is_some() {
            return AnimationState::Thinking;
        }

        AnimationState::from_streaming(
            self.is_streaming,
            has_visible_content,
            has_tools_in_turn,
            active_tools,
            !self.message_queue.is_empty(),
        )
    }

    pub(super) fn theme_kind(&self) -> ThemeKind {
        ThemeKind {
            is_light: self.theme.bg == Theme::light().bg,
        }
    }

    pub(super) fn chat_render_cache_key(
        &self,
        width: u16,
        chat_tool_focus: Option<usize>,
        chat_tool_display: imp_core::config::ChatToolDisplay,
        activity_state: AnimationState,
    ) -> ChatRenderCacheKey {
        ChatRenderCacheKey {
            width,
            messages_epoch: self.chat_render_epoch,
            chat_tool_focus,
            word_wrap: self.config.ui.word_wrap,
            chat_tool_display,
            thinking_lines: self.config.ui.thinking_lines,
            show_timestamps: self.config.ui.show_timestamps,
            animation_level: self.config.ui.animations,
            activity_state,
            theme: self.theme_kind(),
            tick: self.tick,
        }
    }

    pub(super) fn cached_chat_render(
        &mut self,
        width: u16,
        chat_tool_focus: Option<usize>,
        chat_tool_display: imp_core::config::ChatToolDisplay,
        activity_state: AnimationState,
    ) -> &crate::views::chat::ChatRenderData {
        let key =
            self.chat_render_cache_key(width, chat_tool_focus, chat_tool_display, activity_state);
        let cache_hit = self
            .chat_render_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key);
        if !cache_hit {
            let render = build_chat_render_data(
                &self.messages,
                &self.theme,
                &self.highlighter,
                width as usize,
                self.tick,
                chat_tool_focus,
                self.config.ui.word_wrap,
                chat_tool_display,
                self.config.ui.thinking_lines,
                self.config.ui.show_timestamps,
                self.config.ui.animations,
                activity_state,
            );
            self.chat_render_cache = Some(ChatRenderCache { key, render });
        }

        &self
            .chat_render_cache
            .as_ref()
            .expect("chat render cache set")
            .render
    }

    pub(super) fn invalidate_chat_render_cache(&mut self) {
        self.chat_render_cache = None;
        bump_epoch(&mut self.chat_render_epoch);
        self.sidebar_stream_cache = None;
        self.sidebar_detail_cache = None;
    }

    pub(super) fn sidebar_stream_cache_key(&self, width: u16) -> SidebarStreamCacheKey {
        SidebarStreamCacheKey {
            width,
            messages_epoch: self.chat_render_epoch,
            selected: self.tool_focus,
            word_wrap: self.config.ui.word_wrap,
            tool_output: self.config.ui.tool_output,
            tool_output_lines: self.config.ui.tool_output_lines,
            animation_level: self.config.ui.animations,
            theme: self.theme_kind(),
        }
    }

    pub(super) fn cached_sidebar_stream_lines(&mut self, width: u16) -> &Vec<Line<'static>> {
        let key = self.sidebar_stream_cache_key(width);
        let cache_hit = self
            .sidebar_stream_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key);
        if !cache_hit {
            let all_tool_calls: Vec<&DisplayToolCall> = self
                .messages
                .iter()
                .flat_map(|m| m.tool_calls.iter())
                .collect();
            let lines = build_stream_lines(
                &all_tool_calls,
                self.tool_focus,
                &self.theme,
                &self.highlighter,
                self.tick,
                &self.config.ui,
                self.config.ui.animations,
                width as usize,
            );
            self.sidebar_stream_cache = Some(SidebarStreamCache { key, lines });
        }
        &self
            .sidebar_stream_cache
            .as_ref()
            .expect("sidebar stream cache set")
            .lines
    }

    pub(super) fn sidebar_detail_cache_key(
        &self,
        width: u16,
        selected_tc: Option<&DisplayToolCall>,
        thinking: Option<&str>,
        _run: Option<&WorkflowRunSummary>,
    ) -> SidebarDetailCacheKey {
        SidebarDetailCacheKey {
            width,
            messages_epoch: self.chat_render_epoch,
            selected_tool_id_hash: stable_hash(&selected_tc.map(|tc| &tc.id)),
            thinking_hash: stable_hash(&thinking),
            run_hash: {
                #[cfg(feature = "mana-ui")]
                {
                    stable_hash(&_run.map(workflow_run_summary_cache_key))
                }
                #[cfg(not(feature = "mana-ui"))]
                {
                    0
                }
            },
            word_wrap: self.config.ui.word_wrap,
            tool_output_lines: self.config.ui.tool_output_lines,
            animation_level: self.config.ui.animations,
            theme: self.theme_kind(),
        }
    }

    pub(super) fn begin_llm_thought_segment(&mut self) {
        self.llm_thought_segment_started_at = Some(Instant::now());
    }

    pub(super) fn finalize_llm_thought_segment(&mut self) -> Option<u64> {
        self.llm_thought_segment_started_at
            .take()
            .map(|started_at| started_at.elapsed().as_secs().max(1))
    }

    pub(super) fn cached_sidebar_detail_render(
        &mut self,
        width: u16,
        selected_tc: Option<&DisplayToolCall>,
        thinking: Option<&str>,
        run: Option<&WorkflowRunSummary>,
    ) -> &SidebarDetailRenderData {
        let key = self.sidebar_detail_cache_key(width, selected_tc, thinking, run);
        let cache_hit = self
            .sidebar_detail_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key);
        if !cache_hit {
            let render = if let Some(run) = run {
                workflow_run_detail_render_data(run, &self.theme)
            } else if let Some(thinking) = thinking {
                super::thinking_detail_render_data(
                    thinking,
                    &self.theme,
                    width as usize,
                    self.config.ui.word_wrap,
                )
            } else {
                build_detail_render_data(
                    selected_tc,
                    &self.config.ui,
                    &self.highlighter,
                    &self.theme,
                    width as usize,
                )
            };
            self.sidebar_detail_cache = Some(SidebarDetailCache { key, render });
        }
        &self
            .sidebar_detail_cache
            .as_ref()
            .expect("sidebar detail cache set")
            .render
    }

    pub(super) fn latest_thinking_trace(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find_map(|message| {
                message
                    .thinking
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
            })
            .map(str::to_owned)
    }

    pub(super) fn startup_skills(&self) -> Vec<imp_core::resources::Skill> {
        self.startup_surface_metadata.skills.clone()
    }

    pub(super) fn startup_skill_hits(&self, chat_area: Rect) -> Vec<StartupSkillHit> {
        let startup = self.build_startup_surface();
        startup_skill_hits(chat_area, &startup.panel)
    }

    pub(super) fn startup_workflow_hits(&self, chat_area: Rect) -> Vec<StartupWorkflowHit> {
        let startup = self.build_startup_surface();
        startup_workflow_hits(chat_area, &startup.panel)
    }

    pub(super) fn select_startup_workflow_at(&mut self, col: u16, row: u16) -> bool {
        if !matches!(self.mode, UiMode::Normal) || !self.messages.is_empty() {
            return false;
        }

        let Some(chat_area) = self.chat_surface.as_ref().map(|surface| surface.rect) else {
            return false;
        };

        let Some(hit) = self
            .startup_workflow_hits(chat_area)
            .into_iter()
            .find(|hit| point_in_rect(col, row, Some(hit.rect)))
        else {
            return false;
        };

        let Some(workflow) = self
            .startup_surface_metadata
            .workflows
            .get(hit.index)
            .cloned()
        else {
            return false;
        };

        self.selected_startup_workflow = Some(workflow);
        self.selected_startup_skill = None;
        self.sidebar.open = true;
        self.sidebar.reset_detail_scroll();
        self.sidebar_auto_follow = false;
        self.tool_focus = None;
        self.tool_focus_pinned = false;
        self.sidebar_detail_cache = None;
        true
    }

    pub(super) fn select_startup_skill_at(&mut self, col: u16, row: u16) -> bool {
        if !matches!(self.mode, UiMode::Normal) || !self.messages.is_empty() {
            return false;
        }

        let Some(chat_area) = self.chat_surface.as_ref().map(|surface| surface.rect) else {
            return false;
        };

        let Some(hit) = self
            .startup_skill_hits(chat_area)
            .into_iter()
            .find(|hit| point_in_rect(col, row, Some(hit.rect)))
        else {
            return false;
        };

        let Some(skill) = self.startup_skills().into_iter().nth(hit.index) else {
            return false;
        };

        self.selected_startup_skill = Some(skill);
        self.selected_startup_workflow = None;
        self.sidebar.open = true;
        self.sidebar.reset_detail_scroll();
        self.sidebar_auto_follow = false;
        self.tool_focus = None;
        self.tool_focus_pinned = false;
        self.sidebar_detail_cache = None;
        true
    }

    pub(super) fn load_startup_surface_metadata(
        cwd: &Path,
        config: &imp_core::config::Config,
        model_registry: &ModelRegistry,
        model_name: &str,
    ) -> StartupSurfaceMetadata {
        let user_config_dir = imp_core::config::Config::user_config_dir();
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store = AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path));
        let provider_meta = model_registry.resolve_meta(model_name, None);
        let provider_id = provider_meta
            .as_ref()
            .map(|meta| meta.provider.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let web_summary = config
            .web
            .search_provider
            .map(|provider| {
                let status = if auth_store.has_credentials(provider.name()) {
                    "ready"
                } else {
                    "needs key"
                };
                format!("{} ({status})", provider.name())
            })
            .unwrap_or_else(|| "disabled".to_string());

        StartupSurfaceMetadata {
            skills: imp_core::resources::discover_skills(cwd, &user_config_dir),
            workflows: discover_startup_workflows(cwd),
            provider_id,
            web_summary,
            repo_stats: Some(RepoStatsState::Scanning),
            rule_files: discover_rule_files(cwd),
        }
    }

    pub(super) fn build_startup_surface(&self) -> StartupSurfaceData {
        let skills = self.startup_skills();
        let repo_label = self
            .cwd
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("this project")
            .to_string();

        let provider_id = self.startup_surface_metadata.provider_id.as_str();
        let web_summary = strip_status_suffix(&self.startup_surface_metadata.web_summary);
        let mode = format!("{:?}", self.config.mode).to_lowercase();
        let session_name = self
            .session
            .name()
            .map(str::to_string)
            .or_else(|| self.session.title(48))
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "new chat".to_string());
        let mut session_lines = vec![
            format!("• project: {repo_label}"),
            format!("• session: {session_name}"),
            format!("• provider: {provider_id}"),
            format!("• web: {web_summary}"),
            format!(
                "• repo: {}",
                repo_stats_label(self.startup_surface_metadata.repo_stats.as_ref())
            ),
        ];
        session_lines.extend(rule_file_lines(&self.startup_surface_metadata.rule_files));

        let visible_prompt_tools = {
            let mut registry = imp_core::tools::ToolRegistry::new();
            imp_core::builder::register_native_tools(&mut registry);
            let mut names = registry
                .definitions_for_mode(&self.config.mode)
                .into_iter()
                .map(|def| def.name)
                .collect::<Vec<_>>();
            names.sort();
            names
        };

        let actions = vec![
            StartupAction {
                trigger: "input".to_string(),
                label: String::new(),
                description: "prompt, request, bug, or goal".to_string(),
            },
            StartupAction {
                trigger: "/resume".to_string(),
                label: "sessions".to_string(),
                description: "browse and search saved work".to_string(),
            },
            StartupAction {
                trigger: "/settings".to_string(),
                label: "runtime".to_string(),
                description: format!("model, provider, runtime, thinking ({mode})"),
            },
            StartupAction {
                trigger: "Ctrl+L".to_string(),
                label: "model".to_string(),
                description: "switch model".to_string(),
            },
        ];

        let tool_lines = visible_prompt_tools
            .iter()
            .map(|name| format!("{} {}", tool_display_icon(name), tool_display_name(name)))
            .collect::<Vec<_>>();

        let skill_lines = if skills.is_empty() {
            vec!["• none discovered".to_string()]
        } else {
            skills
                .iter()
                .map(|skill| format!("• {}", skill.name))
                .collect::<Vec<_>>()
        };

        let workflow_lines = workflow_startup_lines(&self.startup_surface_metadata.workflows);

        let sections = vec![
            StartupSection {
                title: "session".to_string(),
                lines: session_lines,
            },
            StartupSection {
                title: format!(
                    "workflows · {} found",
                    self.startup_surface_metadata.workflows.len()
                ),
                lines: workflow_lines,
            },
            StartupSection {
                title: "tools".to_string(),
                lines: tool_lines,
            },
            StartupSection {
                title: format!("skills · {} installed", skills.len()),
                lines: skill_lines,
            },
        ];

        StartupSurfaceData {
            panel: StartupPanelData { actions, sections },
        }
    }

    pub(super) fn startup_workflow_detail_render(
        &mut self,
        workflow: &StartupWorkflowItem,
    ) -> SidebarDetailRenderData {
        startup_workflow_detail_render_data(workflow, &self.theme)
    }

    pub(super) fn startup_skill_detail_render(
        &mut self,
        skill: &imp_core::resources::Skill,
    ) -> SidebarDetailRenderData {
        let theme = self.theme_kind();
        if let Some(cache) = self.startup_skill_detail_cache.as_ref() {
            if cache.skill_path == skill.path && cache.theme == theme {
                return cache.render.clone();
            }
        }

        let render = startup_skill_detail_render_data(skill, &self.theme);
        self.startup_skill_detail_cache = Some(StartupSkillDetailCache {
            skill_path: skill.path.clone(),
            theme,
            render: render.clone(),
        });
        render
    }

    pub(super) fn render(&mut self, frame: &mut Frame) {
        self.refresh_render_caches();
        let area = frame.area();
        frame.render_widget(Clear, area);

        // Editor/prompt height: while asking, the prompt box becomes the ask box.
        // Otherwise it grows to fit wrapped prompt text while preserving at least
        // 3 lines for the chat area.
        let editor_inner_width = area.width.saturating_sub(2).max(1);
        let desired_editor_height = if let Some(state) = self.ask_state.as_ref() {
            state.prompt_height(editor_inner_width)
        } else {
            self.editor
                .visual_line_count_with_summary(editor_inner_width, true) as u16
                + 2
        };
        let max_editor_height = area.height.saturating_sub(3).max(3);
        let editor_height = desired_editor_height.clamp(3, max_editor_height);

        let constraints = vec![
            Constraint::Min(3),                // messages area
            Constraint::Length(editor_height), // editor / ask prompt
        ];

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        let (chat_area, editor_area) = (chunks[0], chunks[1]);

        // Split chat area for sidebar when open
        let (chat_area, sidebar_area) = if self.sidebar.open && chat_area.width >= 60 {
            let min_sidebar = 30u16;
            let pct = self.config.ui.sidebar_width.clamp(20, 80);
            let sidebar_w = (chat_area.width * pct / 100)
                .max(min_sidebar)
                .min(chat_area.width.saturating_sub(30));
            let chat_w = chat_area.width.saturating_sub(sidebar_w);
            let chat_rect = Rect {
                width: chat_w,
                ..chat_area
            };
            let sidebar_rect = Rect {
                x: chat_area.x + chat_w,
                width: sidebar_w,
                ..chat_area
            };
            (chat_rect, Some(sidebar_rect))
        } else {
            (chat_area, None)
        };
        let _ = self.theme_kind();

        // Messages
        let chat_tool_display = self.config.ui.effective_chat_tool_display();
        let chat_tool_focus = self.tool_focus;
        let activity_state = self.current_activity_state();
        let total_chat_lines = {
            let chat_render = self.cached_chat_render(
                chat_area.width,
                chat_tool_focus,
                chat_tool_display,
                activity_state,
            );
            chat_render.lines.len()
        };
        self.scroll_offset =
            clamped_scroll_offset_for_total_lines(total_chat_lines, chat_area, self.scroll_offset);
        if self.auto_scroll {
            if let Some(anchor_index) = self.streaming_anchor_user_index {
                self.scroll_offset = scroll_offset_for_message_at_top(
                    &self.messages,
                    &self.theme,
                    &self.highlighter,
                    chat_area,
                    anchor_index,
                    self.tick,
                    chat_tool_focus,
                    self.config.ui.word_wrap,
                    chat_tool_display,
                    self.config.ui.thinking_lines,
                    self.config.ui.show_timestamps,
                    self.config.ui.animations,
                    activity_state,
                );
            }
        }
        if self.scroll_offset == 0 {
            self.auto_scroll = true;
        }

        let chat_lines = {
            self.cached_chat_render(
                chat_area.width,
                chat_tool_focus,
                chat_tool_display,
                activity_state,
            )
            .lines
            .clone()
        };

        if matches!(self.mode, UiMode::Normal) && self.messages.is_empty() {
            let startup = self.build_startup_surface();
            frame.render_widget(
                StartupPanelView::new(&startup.panel, &self.theme),
                chat_area,
            );
            self.chat_surface = Some(TextSurface::new(
                SelectablePane::Chat,
                chat_area,
                Vec::new(),
                0,
            ));
            self.chat_tool_click_map.clear();
        } else {
            let chat = RenderedChatView::new(&chat_lines).scroll(self.scroll_offset);
            frame.render_widget(chat, chat_area);

            self.chat_surface = Some(build_text_surface_from_lines(
                &chat_lines,
                chat_area,
                self.scroll_offset,
            ));
            self.chat_tool_click_map =
                build_click_map_from_rendered_lines(&chat_lines, chat_area, self.scroll_offset);
        }

        if !matches!(self.mode, UiMode::Normal) || !self.messages.is_empty() {
            self.selected_startup_skill = None;
            self.selected_startup_workflow = None;
        }

        // Sidebar
        if let Some(sidebar_area) = sidebar_area {
            let tc_count = self.total_tool_calls();
            let sub = sidebar_sub_areas(sidebar_area, tc_count, self.config.ui.sidebar_style);
            let stream_lines =
                if self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Stream {
                    Some(self.cached_sidebar_stream_lines(sub.0.width).clone())
                } else {
                    None
                };
            let selected_index = if self.selected_startup_skill.is_some()
                || self.selected_startup_workflow.is_some()
            {
                None
            } else {
                self.tool_focus.or_else(|| {
                    (self.config.ui.sidebar_style == imp_core::config::SidebarStyle::Inspector)
                        .then(|| self.total_tool_calls().checked_sub(1))
                        .flatten()
                })
            };
            let detail_render = if let Some(workflow) = self.selected_startup_workflow.clone() {
                Some(self.startup_workflow_detail_render(&workflow))
            } else if let Some(skill) = self.selected_startup_skill.clone() {
                Some(self.startup_skill_detail_render(&skill))
            } else if matches!(
                self.config.ui.sidebar_style,
                imp_core::config::SidebarStyle::Split | imp_core::config::SidebarStyle::Inspector
            ) {
                let selected_tc_owned = self.selected_tool_call();
                let run = if selected_tc_owned.is_none() {
                    {
                        #[cfg(feature = "mana-ui")]
                        {
                            self.active_workflow_run.clone()
                        }
                        #[cfg(not(feature = "mana-ui"))]
                        {
                            None
                        }
                    }
                } else {
                    None
                };
                let thinking = (selected_tc_owned.is_none() && run.is_none())
                    .then(|| self.latest_thinking_trace())
                    .flatten();
                Some(
                    self.cached_sidebar_detail_render(
                        sub.1.width,
                        selected_tc_owned.as_ref(),
                        thinking.as_deref(),
                        run.as_ref(),
                    )
                    .clone(),
                )
            } else {
                None
            };

            let all_tool_calls: Vec<&DisplayToolCall> = self
                .messages
                .iter()
                .flat_map(|m| m.tool_calls.iter())
                .collect();
            let mut view = SidebarView::new(
                all_tool_calls,
                selected_index,
                &self.theme,
                &self.highlighter,
                self.tick,
                self.sidebar.list_scroll,
                self.sidebar.detail_scroll,
                &self.config.ui,
            );

            match self.config.ui.sidebar_style {
                imp_core::config::SidebarStyle::Inspector => {
                    let detail_lines = detail_render.as_ref().expect("detail cache lines");
                    view = view.precomputed_detail_lines(&detail_lines.lines);
                    frame.render_widget(view, sidebar_area);
                }
                imp_core::config::SidebarStyle::Stream => {
                    let stream_lines = stream_lines.expect("stream cache lines");
                    view = view.precomputed_stream_lines(&stream_lines);
                    frame.render_widget(view, sidebar_area);
                }
                imp_core::config::SidebarStyle::Split => {
                    let detail_lines = detail_render.as_ref().expect("detail cache lines");
                    view = view.precomputed_detail_lines(&detail_lines.lines);
                    frame.render_widget(view, sidebar_area);
                }
            }

            self.sidebar_list_rect = Some(sub.0);
            self.sidebar_detail_rect = Some(sub.1);
            self.sidebar.list_height = sub.0.height;
            let detail_plain_lines = detail_render
                .as_ref()
                .map(|render| render.plain_lines.as_slice())
                .unwrap_or(&[]);
            self.sidebar_detail_surface = Some(build_detail_text_surface_from_plain_lines(
                detail_plain_lines,
                sub.1,
                self.sidebar.detail_scroll,
            ));
        } else {
            self.sidebar_list_rect = None;
            self.sidebar_detail_rect = None;
            self.sidebar_detail_surface = None;
        }

        // Prompt area: reuse the normal editor box for asks.
        if let Some(ref state) = self.ask_state {
            use crate::views::ask_bar::AskBar;
            frame.render_widget(AskBar::new(state, &self.theme), editor_area);
        } else {
            let status_info = self.build_status_info();
            let git_label = self.cached_git_label();
            let active_context_window = self.active_context_window();
            let editor = EditorView::new(&self.editor, &self.theme, self.thinking_level)
                .summarize_paste(true)
                .model(&self.model_name)
                .identity(&status_info.cwd, &status_info.session_name)
                .turn_elapsed(status_info.turn_elapsed)
                .extension_items(&status_info.extension_items, status_info.peek)
                .streaming(self.is_streaming)
                .queued(self.queued_message_preview(area.width))
                .context_usage(
                    self.estimated_active_context_tokens(),
                    active_context_window,
                    self.config.ui.show_context_usage,
                )
                .tick(self.tick)
                .animation_level(self.config.ui.animations)
                .activity_state(activity_state)
                .workflow_mode(self.workflow_mode)
                .workflow_scope_label(self.active_workflow_scope_label())
                .workflow_run_label(self.active_workflow_run_label())
                .loop_label(self.loop_label())
                .git_label(git_label);
            frame.render_widget(editor, editor_area);
        }

        frame.render_widget(
            SelectionOverlay::new(
                &self.theme,
                self.selection.as_ref(),
                self.chat_surface.as_ref(),
                self.sidebar_detail_surface.as_ref(),
            ),
            area,
        );

        // Pre-render: clamp session picker scroll so selected item is visible
        if let UiMode::SessionPicker(ref mut sp) = self.mode {
            let overlay_area = centered_rect(75, 70, area);
            let inner_h = overlay_area.height.saturating_sub(2) as usize;
            let visible_rows = (inner_h / 3).max(1);
            sp.clamp_scroll(visible_rows);
        }

        // Render overlays
        match &self.mode {
            UiMode::Normal => {}
            UiMode::ModelSelector(state) => {
                let overlay_area = centered_rect(60, 70, area);
                let view = ModelSelectorView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
            UiMode::CommandPalette(state) => {
                let palette_area = command_dropdown_area(editor_area, 12);
                let view = CommandPaletteView::new(state, &self.theme);
                frame.render_widget(view, palette_area);
            }
            UiMode::LoginPicker(state) => {
                let overlay_area = centered_rect(60, 40, area);
                let view = LoginPickerView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
            UiMode::SecretsPicker(state) => {
                let overlay_area = centered_rect(70, 50, area);
                let view = SecretsPickerView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
            #[cfg(feature = "mana-ui")]
            UiMode::ManaNavigator(state) => {
                let mana_area = centered_rect(88, 86, area);
                let view = ManaNavigatorView::new(state, &self.theme);
                frame.render_widget(view, mana_area);
            }
            UiMode::TreeView(state) => {
                let tree_area = centered_rect(80, 80, area);
                let view = TreeView::new(state, &self.theme);
                frame.render_widget(view, tree_area);
            }
            UiMode::Settings(state) => {
                let overlay_area = centered_rect(80, 90, area);
                let view = SettingsView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
            UiMode::SessionPicker(state) => {
                let overlay_area = centered_rect(75, 70, area);
                let view = SessionPickerView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
            UiMode::Welcome(state) => {
                let overlay_area = centered_rect(70, 80, area);
                let view = WelcomeView::new(state, &self.theme);
                frame.render_widget(view, overlay_area);
            }
        }

        // Set cursor position (only in normal mode)
        if matches!(self.mode, UiMode::Normal) {
            let (cx, cy) = if let Some(state) = self.ask_state.as_ref() {
                state.cursor_screen_position(editor_area)
            } else {
                self.editor.cursor_screen_position(editor_area)
            };
            frame.set_cursor_position((cx, cy));
        }
    }

    pub(super) fn cached_git_label(&mut self) -> Option<String> {
        const GIT_LABEL_CACHE_TTL: Duration = Duration::from_secs(2);

        let now = Instant::now();
        let cache_hit = self.git_label_cache.as_ref().is_some_and(|cache| {
            cache.cwd == self.cwd && now.duration_since(cache.refreshed_at) < GIT_LABEL_CACHE_TTL
        });
        if cache_hit
            || self.is_streaming
            || self.compaction_task.is_some()
            || self.lua_command_task.is_some()
        {
            return self
                .git_label_cache
                .as_ref()
                .and_then(|cache| (cache.cwd == self.cwd).then(|| cache.label.clone()))
                .flatten();
        }

        let label = compact_git_label(&self.cwd);
        self.git_label_cache = Some(GitLabelCache {
            cwd: self.cwd.clone(),
            refreshed_at: now,
            label: label.clone(),
        });
        label
    }

    fn refresh_render_caches(&mut self) {
        if self.current_oauth_display_info_model != self.model_name {
            self.current_oauth_display_info = self.load_current_oauth_display_info();
            self.current_oauth_display_info_model = self.model_name.clone();
        }
        if self.current_model_meta_for_persistence_model != self.model_name {
            self.current_model_meta_for_persistence =
                self.load_current_model_meta_for_persistence();
            self.current_model_meta_for_persistence_model = self.model_name.clone();
        }
    }

    fn load_current_model_meta_for_persistence(&self) -> Option<ModelMeta> {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store = AuthStore::load(&auth_path).ok();
        let mut meta = self.model_registry.resolve_meta(&self.model_name, None)?;

        if let Some(auth_store) = auth_store.as_ref() {
            if should_use_chatgpt_provider(auth_store, &self.model_registry, &meta) {
                meta = self
                    .model_registry
                    .resolve_meta(&self.model_name, Some("openai-codex"))?;
            }
        }

        Some(meta)
    }

    fn load_current_oauth_display_info(&self) -> Option<imp_llm::auth::OAuthDisplayInfo> {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store = AuthStore::load(&auth_path).ok()?;
        let meta = self.model_registry.resolve_meta(&self.model_name, None)?;
        let mut provider_name = meta.provider.clone();
        if should_use_chatgpt_provider(&auth_store, &self.model_registry, &meta) {
            provider_name = "openai-codex".to_string();
        }
        auth_store.oauth_display_info(&provider_name)
    }

    pub(super) fn build_status_info(&self) -> StatusInfo {
        let cwd = self.cwd.to_string_lossy().to_string();
        let session_name = self
            .session
            .name()
            .map(str::to_string)
            .or_else(|| self.session.title(48))
            .unwrap_or_default();

        let total_input = self.accumulated_usage.input_tokens;
        let total_output = self.accumulated_usage.output_tokens;
        let current_context_tokens = self.display_context_tokens();
        let context_window = self.active_context_window();
        // Show the active-history estimate against the same display/input budget
        // used by runtime preflight. GPT-5.5 displays a rounded 1.0M window
        // while keeping a small reserve inside its 1.05M total window.
        let context_percent = if context_window > 0 {
            current_context_tokens as f64 / context_window as f64
        } else {
            0.0
        };
        let mut extension_items = self.status_items.clone();
        if !self.verification_status_items.is_empty() {
            extension_items.insert(
                "verify".into(),
                self.verification_status_items
                    .values()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        if let Some(info) = self.current_oauth_display_info() {
            extension_items.insert("oauth".into(), info.status_summary());
        }
        let active_tools = self
            .messages
            .iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter(|tc| tc.output.is_none() && !tc.is_error)
            .count() as u32;

        StatusInfo {
            cwd,
            session_name,
            model: self.model_name.clone(),
            thinking: format!("{:?}", self.thinking_level),
            input_tokens: total_input,
            output_tokens: total_output,
            current_context_tokens,
            cost: self.accumulated_cost.total,
            context_percent,
            context_window,
            show_cost: self.config.ui.show_cost,
            show_context_usage: self.config.ui.show_context_usage,
            peek: self.tools_expanded,
            extension_items,
            is_streaming: self.is_streaming,
            active_tools,
            turn_elapsed: (self.is_streaming || self.agent_start_task.is_some())
                .then(|| self.turn_tracker.elapsed()),
            tick: self.tick,
            animation_level: self.config.ui.animations,
            activity_state: self.current_activity_state(),
        }
    }

    fn current_oauth_display_info(&self) -> Option<imp_llm::auth::OAuthDisplayInfo> {
        self.current_oauth_display_info.clone()
    }

    pub(super) fn current_model_meta_for_persistence(&self) -> Option<ModelMeta> {
        self.current_model_meta_for_persistence.clone()
    }

    #[cfg(feature = "mana-ui")]
    fn handle_mana_navigator_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                self.mode = UiMode::Normal;
            }
            KeyCode::Up => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.move_up();
                }
            }
            KeyCode::Char('k') => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    if state.filter().is_empty() {
                        state.move_up();
                    } else {
                        state.push_filter_char('k');
                    }
                }
            }
            KeyCode::Down => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.move_down();
                }
            }
            KeyCode::Char('j') => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    if state.filter().is_empty() {
                        state.move_down();
                    } else {
                        state.push_filter_char('j');
                    }
                }
            }
            KeyCode::Left => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.collapse_selected();
                }
            }
            KeyCode::Char('h') => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    if state.filter().is_empty() {
                        state.collapse_selected();
                    } else {
                        state.push_filter_char('h');
                    }
                }
            }
            KeyCode::Right => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.expand_selected();
                }
            }
            KeyCode::Char('l') => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    if state.filter().is_empty() {
                        state.expand_selected();
                    } else {
                        state.push_filter_char('l');
                    }
                }
            }
            KeyCode::Enter => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.toggle_selected();
                }
            }
            KeyCode::PageUp => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.scroll_detail_up();
                }
            }
            KeyCode::PageDown => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.scroll_detail_down();
                }
            }
            KeyCode::Backspace => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.pop_filter_char();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.clear_filter();
                }
            }
            KeyCode::Char(ch) => {
                if let UiMode::ManaNavigator(ref mut state) = self.mode {
                    state.push_filter_char(ch);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_tree_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                self.mode = UiMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let UiMode::TreeView(ref mut state) = self.mode {
                    state.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let UiMode::TreeView(ref mut state) = self.mode {
                    state.move_down();
                }
            }
            KeyCode::Enter => {
                let selected_id = if let UiMode::TreeView(ref state) = self.mode {
                    state.selected_id().map(String::from)
                } else {
                    None
                };
                if let Some(id) = selected_id {
                    let _ = self.session.navigate(&id);
                    self.load_session_messages();
                    self.mode = UiMode::Normal;
                }
            }
            KeyCode::Char('f') => {
                let selected_id = if let UiMode::TreeView(ref state) = self.mode {
                    state.selected_id().map(String::from)
                } else {
                    None
                };
                if let Some(id) = selected_id {
                    let path = imp_core::storage::global_sessions_dir()
                        .join(format!("{}.jsonl", uuid::Uuid::new_v4()));
                    match self.session.fork(&id, &path) {
                        Ok(forked) => {
                            self.session = forked;
                            self.load_session_messages();
                            self.mode = UiMode::Normal;
                            self.push_system_msg(
                                "Forked from selected tree node. You're on a new branch.",
                            );
                        }
                        Err(e) => {
                            self.mode = UiMode::Normal;
                            self.push_system_msg(&format!("Fork failed: {e}"));
                        }
                    }
                }
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let UiMode::TreeView(ref mut state) = self.mode {
                    state.cycle_filter();
                }
            }
            _ => {}
        }
    }

    // ── Tool focus helpers ───────────────────────────────────────
}
