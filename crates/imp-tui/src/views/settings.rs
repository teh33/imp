use imp_core::config::{
    AgentMode, AnimationLevel, ChatToolDisplay, Config, ContextConfig, ContinuePolicy, LuaConfig,
    ShellBackend, SidebarStyle, ToolOutputDisplay, WorkflowConfig, WorkflowRunConfig,
    WorkflowScopePreference, WriteOverwritePolicy,
};
use imp_core::tools::web::types::SearchProvider;
use imp_llm::auth::AuthStore;
use imp_llm::model::ModelMeta;
use imp_llm::ThinkingLevel;

mod layout;
mod options;
mod render;

#[cfg(test)]
pub(crate) use layout::{selected_settings_row, settings_scroll_offset, total_settings_rows};
use options::{next_thinking, prev_thinking, theme_options};

/// Which field in the settings panel is focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Model,
    ChosenModels,
    Theme,
    ThinkingLevel,
    MaxTokens,
    MaxTurns,
    ObservationMask,
    ReadMaxLines,
    SidebarWidth,
    WordWrap,
    Animations,
    AutoOpenSidebar,
    SidebarAutoOpenWidth,
    ThinkingLines,
    StreamingLines,
    MouseScrollLines,
    KeyboardScrollLines,
    ShowTimestamps,
    ShowCost,
    ShowContextUsage,
    NotifyOnAgentComplete,
    ContinuePolicy,
    AgentMode,
    WriteOverwritePolicy,
    ShellBackend,
    LuaNativeTools,
    LuaShellExec,
    LuaHttp,
    LuaSecrets,
    ImproveAutoTurnBudget,
    LoopTurnBudget,
    WebSearchProvider,
    TavilyApiKey,
    ExaApiKey,
    WorkflowScope,
    WorkflowAutoCommit,
    WorkflowAutoCloseParent,
    WorkflowVerifyTimeout,
    WorkflowRunBackground,
    WorkflowMaxWorkers,
    WorkflowReviewAfterRun,
    WorkflowContinueAfterFailure,
    Save,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Model,
    Ui,
    Security,
    Web,
    Workflow,
}

const SETTINGS_TABS: &[SettingsTab] = &[
    SettingsTab::General,
    SettingsTab::Model,
    SettingsTab::Ui,
    SettingsTab::Security,
    SettingsTab::Web,
    SettingsTab::Workflow,
];

const GENERAL_FIELDS: &[SettingsField] = &[
    SettingsField::Theme,
    SettingsField::MaxTurns,
    SettingsField::NotifyOnAgentComplete,
    SettingsField::ContinuePolicy,
    SettingsField::ImproveAutoTurnBudget,
    SettingsField::LoopTurnBudget,
];

const MODEL_FIELDS: &[SettingsField] = &[
    SettingsField::Model,
    SettingsField::ChosenModels,
    SettingsField::ThinkingLevel,
    SettingsField::MaxTokens,
    SettingsField::ObservationMask,
];

const UI_FIELDS: &[SettingsField] = &[
    SettingsField::ReadMaxLines,
    SettingsField::SidebarWidth,
    SettingsField::WordWrap,
    SettingsField::Animations,
    SettingsField::AutoOpenSidebar,
    SettingsField::SidebarAutoOpenWidth,
    SettingsField::ThinkingLines,
    SettingsField::StreamingLines,
    SettingsField::MouseScrollLines,
    SettingsField::KeyboardScrollLines,
    SettingsField::ShowTimestamps,
    SettingsField::ShowCost,
    SettingsField::ShowContextUsage,
];

const SECURITY_FIELDS: &[SettingsField] = &[
    SettingsField::AgentMode,
    SettingsField::WriteOverwritePolicy,
    SettingsField::ShellBackend,
    SettingsField::LuaNativeTools,
    SettingsField::LuaShellExec,
    SettingsField::LuaHttp,
    SettingsField::LuaSecrets,
];

const WEB_FIELDS: &[SettingsField] = &[
    SettingsField::WebSearchProvider,
    SettingsField::TavilyApiKey,
    SettingsField::ExaApiKey,
];

const WORKFLOW_FIELDS: &[SettingsField] = &[
    SettingsField::WorkflowScope,
    SettingsField::WorkflowAutoCommit,
    SettingsField::WorkflowAutoCloseParent,
    SettingsField::WorkflowVerifyTimeout,
    SettingsField::WorkflowRunBackground,
    SettingsField::WorkflowMaxWorkers,
    SettingsField::WorkflowReviewAfterRun,
    SettingsField::WorkflowContinueAfterFailure,
];

const FIELDS: &[SettingsField] = &[
    SettingsField::Model,
    SettingsField::ChosenModels,
    SettingsField::Theme,
    SettingsField::ThinkingLevel,
    SettingsField::MaxTokens,
    SettingsField::MaxTurns,
    SettingsField::ObservationMask,
    SettingsField::ReadMaxLines,
    SettingsField::SidebarWidth,
    SettingsField::WordWrap,
    SettingsField::Animations,
    SettingsField::AutoOpenSidebar,
    SettingsField::SidebarAutoOpenWidth,
    SettingsField::ThinkingLines,
    SettingsField::StreamingLines,
    SettingsField::MouseScrollLines,
    SettingsField::KeyboardScrollLines,
    SettingsField::ShowTimestamps,
    SettingsField::ShowCost,
    SettingsField::ShowContextUsage,
    SettingsField::NotifyOnAgentComplete,
    SettingsField::ContinuePolicy,
    SettingsField::AgentMode,
    SettingsField::WriteOverwritePolicy,
    SettingsField::ShellBackend,
    SettingsField::LuaNativeTools,
    SettingsField::LuaShellExec,
    SettingsField::LuaHttp,
    SettingsField::LuaSecrets,
    SettingsField::ImproveAutoTurnBudget,
    SettingsField::LoopTurnBudget,
    SettingsField::WebSearchProvider,
    SettingsField::TavilyApiKey,
    SettingsField::ExaApiKey,
    SettingsField::WorkflowScope,
    SettingsField::WorkflowAutoCommit,
    SettingsField::WorkflowAutoCloseParent,
    SettingsField::WorkflowVerifyTimeout,
    SettingsField::WorkflowRunBackground,
    SettingsField::WorkflowMaxWorkers,
    SettingsField::WorkflowReviewAfterRun,
    SettingsField::WorkflowContinueAfterFailure,
    SettingsField::Save,
];

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Model => "Model",
            SettingsTab::Ui => "UI",
            SettingsTab::Security => "Security",
            SettingsTab::Web => "Web",
            SettingsTab::Workflow => "Workflow",
        }
    }

    fn fields(self) -> &'static [SettingsField] {
        match self {
            SettingsTab::General => GENERAL_FIELDS,
            SettingsTab::Model => MODEL_FIELDS,
            SettingsTab::Ui => UI_FIELDS,
            SettingsTab::Security => SECURITY_FIELDS,
            SettingsTab::Web => WEB_FIELDS,
            SettingsTab::Workflow => WORKFLOW_FIELDS,
        }
    }

    fn empty_message(self) -> Option<&'static str> {
        match self {
            SettingsTab::Security => None,
            SettingsTab::Workflow => None,
            _ => None,
        }
    }
}

fn field_index(field: SettingsField) -> usize {
    FIELDS
        .iter()
        .position(|candidate| *candidate == field)
        .expect("settings field is registered")
}

/// State for the settings overlay.
#[derive(Debug, Clone)]
pub struct SettingsState {
    pub selected: usize,
    pub tab: SettingsTab,
    pub model: String,
    pub model_options: Vec<String>,
    pub chosen_models: Vec<String>,
    pub theme_name: String,
    pub theme_options: Vec<String>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: u32,
    pub max_turns: u32,
    pub observation_mask: f64,
    pub sidebar_style: SidebarStyle,
    pub tool_output: ToolOutputDisplay,
    pub tool_output_lines: usize,
    pub read_max_lines: usize,
    pub sidebar_width: u16,
    pub word_wrap: bool,
    pub animations: AnimationLevel,
    pub chat_tool_display: ChatToolDisplay,
    pub auto_open_sidebar: bool,
    pub sidebar_auto_open_width: u16,
    pub thinking_lines: usize,
    pub streaming_lines: usize,
    pub mouse_scroll_lines: usize,
    pub keyboard_scroll_lines: usize,
    pub show_timestamps: bool,
    pub show_cost: bool,
    pub show_context_usage: bool,
    pub notify_on_agent_complete: bool,
    pub continue_policy: ContinuePolicy,
    pub agent_mode: AgentMode,
    pub write_overwrite_policy: WriteOverwritePolicy,
    pub shell_backend: ShellBackend,
    pub lua_native_tools: bool,
    pub lua_shell_exec: bool,
    pub lua_http: bool,
    pub lua_secrets: bool,
    pub improve_auto_turn_budget: u32,
    pub loop_turn_budget: u32,
    pub web_search_provider: Option<SearchProvider>,
    pub workflow_scope: WorkflowScopePreference,
    pub workflow_auto_commit: bool,
    pub workflow_auto_close_parent: bool,
    pub workflow_verify_timeout: u64,
    pub workflow_run_background: bool,
    pub workflow_max_workers: u32,
    pub workflow_review_after_run: bool,
    pub workflow_continue_after_failure: bool,
    pub tavily_api_key: String,
    pub exa_api_key: String,
    pub tavily_configured: bool,
    pub exa_configured: bool,
    pub editing_number: bool,
    pub edit_buffer: String,
    pub dirty: bool,
}

impl SettingsState {
    fn normalized_selected(&self) -> usize {
        field_index(self.current_field())
    }

    fn selected_tab_index(&self) -> usize {
        SETTINGS_TABS
            .iter()
            .position(|candidate| *candidate == self.tab)
            .unwrap_or(0)
    }

    fn visible_fields(&self) -> &'static [SettingsField] {
        self.tab.fields()
    }

    fn visible_selection(&self) -> Vec<SettingsField> {
        let mut fields = self.visible_fields().to_vec();
        fields.push(SettingsField::Save);
        fields
    }

    pub fn switch_tab_forward(&mut self) {
        self.commit_edit();
        let next = (self.selected_tab_index() + 1) % SETTINGS_TABS.len();
        self.tab = SETTINGS_TABS[next];
        self.selected = field_index(
            self.visible_fields()
                .first()
                .copied()
                .unwrap_or(SettingsField::Save),
        );
    }

    pub fn switch_tab_backward(&mut self) {
        self.commit_edit();
        let idx = self.selected_tab_index();
        let prev = if idx == 0 {
            SETTINGS_TABS.len() - 1
        } else {
            idx - 1
        };
        self.tab = SETTINGS_TABS[prev];
        self.selected = field_index(
            self.visible_fields()
                .first()
                .copied()
                .unwrap_or(SettingsField::Save),
        );
    }

    pub fn new(
        config: &Config,
        model_name: &str,
        models: &[ModelMeta],
        auth_store: &AuthStore,
    ) -> Self {
        Self {
            selected: field_index(SettingsField::Theme),
            tab: SettingsTab::General,
            model: model_name.to_string(),
            model_options: models.iter().map(|m| m.id.clone()).collect(),
            chosen_models: config.enabled_models.clone().unwrap_or_default(),
            theme_name: config.theme.clone().unwrap_or_else(|| "default".into()),
            theme_options: theme_options(config.theme.as_deref()),
            thinking_level: config.thinking.unwrap_or(ThinkingLevel::Medium),
            max_tokens: config.max_tokens.unwrap_or(4096),
            max_turns: config.max_turns.unwrap_or(100),
            observation_mask: config.context.observation_mask_threshold,
            sidebar_style: config.ui.sidebar_style,
            tool_output: config.ui.tool_output,
            tool_output_lines: config.ui.tool_output_lines,
            read_max_lines: config.ui.read_max_lines,
            sidebar_width: config.ui.sidebar_width,
            word_wrap: config.ui.word_wrap,
            animations: config.ui.animations,
            chat_tool_display: config.ui.effective_chat_tool_display(),
            auto_open_sidebar: config.ui.auto_open_sidebar,
            sidebar_auto_open_width: config.ui.sidebar_auto_open_width,
            thinking_lines: config.ui.thinking_lines,
            streaming_lines: config.ui.streaming_lines,
            mouse_scroll_lines: config.ui.mouse_scroll_lines,
            keyboard_scroll_lines: config.ui.keyboard_scroll_lines,
            show_timestamps: config.ui.show_timestamps,
            show_cost: config.ui.show_cost,
            show_context_usage: config.ui.show_context_usage,
            notify_on_agent_complete: config.ui.notify_on_agent_complete,
            continue_policy: config.ui.continue_policy,
            agent_mode: config.mode,
            write_overwrite_policy: config.write.overwrite_policy,
            shell_backend: config.shell.backend.clone(),
            lua_native_tools: config
                .lua
                .resolve_policy(config.mode)
                .allow_native_tool_calls,
            lua_shell_exec: config.lua.resolve_policy(config.mode).allow_shell_exec,
            lua_http: config.lua.resolve_policy(config.mode).allow_http,
            lua_secrets: config.lua.resolve_policy(config.mode).allow_secrets,
            improve_auto_turn_budget: config.ui.improve_auto_turn_budget,
            loop_turn_budget: config.ui.loop_turn_budget,
            web_search_provider: config.web.search_provider,
            workflow_scope: config.workflow.scope,
            workflow_auto_commit: config.workflow.auto_commit,
            workflow_auto_close_parent: config.workflow.auto_close_parent,
            workflow_verify_timeout: config.workflow.verify_timeout.unwrap_or(0),
            workflow_run_background: config.workflow.run.background,
            workflow_max_workers: config.workflow.run.max_workers,
            workflow_review_after_run: config.workflow.run.review_after_run,
            workflow_continue_after_failure: config.workflow.run.continue_after_failure,
            tavily_api_key: String::new(),
            exa_api_key: String::new(),
            tavily_configured: auth_store.stored.contains_key("tavily")
                || std::env::var("TAVILY_API_KEY").is_ok(),
            exa_configured: auth_store.stored.contains_key("exa")
                || std::env::var("EXA_API_KEY").is_ok(),
            editing_number: false,
            edit_buffer: String::new(),
            dirty: false,
        }
    }

    pub fn current_field(&self) -> SettingsField {
        let selected = FIELDS
            .get(self.selected)
            .copied()
            .unwrap_or(SettingsField::Save);
        if selected == SettingsField::Save || self.visible_fields().contains(&selected) {
            return selected;
        }
        self.visible_fields()
            .first()
            .copied()
            .unwrap_or(SettingsField::Save)
    }

    pub fn move_up(&mut self) {
        self.commit_edit();
        let fields = self.visible_selection();
        let current = self.current_field();
        let pos = fields
            .iter()
            .position(|field| *field == current)
            .unwrap_or(0);
        if pos > 0 {
            self.selected = field_index(fields[pos - 1]);
        }
    }

    pub fn move_down(&mut self) {
        self.commit_edit();
        let fields = self.visible_selection();
        let current = self.current_field();
        let pos = fields
            .iter()
            .position(|field| *field == current)
            .unwrap_or(0);
        if pos + 1 < fields.len() {
            self.selected = field_index(fields[pos + 1]);
        }
    }

    /// Cycle the current field's value forward.
    pub fn cycle_forward(&mut self) {
        self.dirty = true;
        match self.current_field() {
            SettingsField::Model => {
                if !self.model_options.is_empty() {
                    if let Some(idx) = self.model_options.iter().position(|m| *m == self.model) {
                        let next = (idx + 1) % self.model_options.len();
                        self.model = self.model_options[next].clone();
                    }
                }
            }
            SettingsField::ChosenModels => {
                self.toggle_current_model_in_chosen();
            }
            SettingsField::Theme => {
                if !self.theme_options.is_empty() {
                    let idx = self
                        .theme_options
                        .iter()
                        .position(|t| *t == self.theme_name)
                        .unwrap_or(0);
                    let next = (idx + 1) % self.theme_options.len();
                    self.theme_name = self.theme_options[next].clone();
                }
            }
            SettingsField::ThinkingLevel => {
                self.thinking_level = next_thinking(self.thinking_level);
            }
            SettingsField::MaxTokens => {
                self.max_tokens = self.max_tokens.saturating_add(256).min(128_000);
            }
            SettingsField::MaxTurns => {
                self.max_turns = self.max_turns.saturating_add(10);
            }
            SettingsField::ObservationMask => {
                self.observation_mask = (self.observation_mask + 0.05).min(1.0);
            }
            SettingsField::ReadMaxLines => {
                self.read_max_lines = self.read_max_lines.saturating_add(100);
            }
            SettingsField::SidebarWidth => {
                self.sidebar_width = (self.sidebar_width + 5).min(80);
            }
            SettingsField::WordWrap => {
                self.word_wrap = !self.word_wrap;
            }
            SettingsField::Animations => {
                self.animations = match self.animations {
                    AnimationLevel::None => AnimationLevel::Spinner,
                    AnimationLevel::Spinner => AnimationLevel::Minimal,
                    AnimationLevel::Minimal => AnimationLevel::None,
                };
            }
            SettingsField::AutoOpenSidebar => {
                self.auto_open_sidebar = !self.auto_open_sidebar;
            }
            SettingsField::SidebarAutoOpenWidth => {
                self.sidebar_auto_open_width = (self.sidebar_auto_open_width + 10).min(240);
            }
            SettingsField::ThinkingLines => {
                self.thinking_lines = self.thinking_lines.saturating_add(1).min(20);
            }
            SettingsField::StreamingLines => {
                self.streaming_lines = self.streaming_lines.saturating_add(1).min(20);
            }
            SettingsField::MouseScrollLines => {
                self.mouse_scroll_lines = self.mouse_scroll_lines.saturating_add(1).min(20);
            }
            SettingsField::KeyboardScrollLines => {
                self.keyboard_scroll_lines = self.keyboard_scroll_lines.saturating_add(5).min(100);
            }
            SettingsField::ShowTimestamps => {
                self.show_timestamps = !self.show_timestamps;
            }
            SettingsField::ShowCost => {
                self.show_cost = !self.show_cost;
            }
            SettingsField::ShowContextUsage => {
                self.show_context_usage = !self.show_context_usage;
            }
            SettingsField::NotifyOnAgentComplete => {
                self.notify_on_agent_complete = !self.notify_on_agent_complete;
            }
            SettingsField::ContinuePolicy => {
                self.continue_policy = match self.continue_policy {
                    ContinuePolicy::Disabled => ContinuePolicy::Conservative,
                    ContinuePolicy::Conservative => ContinuePolicy::Balanced,
                    ContinuePolicy::Balanced => ContinuePolicy::Aggressive,
                    ContinuePolicy::Aggressive => ContinuePolicy::Disabled,
                };
            }
            SettingsField::AgentMode => {
                self.agent_mode = match self.agent_mode {
                    AgentMode::Full => AgentMode::Worker,
                    AgentMode::Worker => AgentMode::Orchestrator,
                    AgentMode::Orchestrator => AgentMode::Planner,
                    AgentMode::Planner => AgentMode::Reviewer,
                    AgentMode::Reviewer => AgentMode::Auditor,
                    AgentMode::Auditor => AgentMode::Full,
                };
            }
            SettingsField::WriteOverwritePolicy => {
                self.write_overwrite_policy = match self.write_overwrite_policy {
                    WriteOverwritePolicy::Warn => WriteOverwritePolicy::RequireRead,
                    WriteOverwritePolicy::RequireRead => WriteOverwritePolicy::BlockStale,
                    WriteOverwritePolicy::BlockStale => WriteOverwritePolicy::Deny,
                    WriteOverwritePolicy::Deny => WriteOverwritePolicy::Warn,
                };
            }
            SettingsField::ShellBackend => {
                self.shell_backend = match self.shell_backend {
                    ShellBackend::Sh => ShellBackend::Rush,
                    ShellBackend::Rush => ShellBackend::RushDaemon,
                    ShellBackend::RushDaemon => ShellBackend::Sh,
                };
            }
            SettingsField::LuaNativeTools => self.lua_native_tools = !self.lua_native_tools,
            SettingsField::LuaShellExec => self.lua_shell_exec = !self.lua_shell_exec,
            SettingsField::LuaHttp => self.lua_http = !self.lua_http,
            SettingsField::LuaSecrets => self.lua_secrets = !self.lua_secrets,
            SettingsField::ImproveAutoTurnBudget => {
                self.improve_auto_turn_budget =
                    self.improve_auto_turn_budget.saturating_add(1).min(100);
            }
            SettingsField::LoopTurnBudget => {
                self.loop_turn_budget = if self.loop_turn_budget >= 100 {
                    0
                } else {
                    self.loop_turn_budget.saturating_add(1)
                };
            }
            SettingsField::WebSearchProvider => {
                self.web_search_provider = match self.web_search_provider {
                    None => Some(SearchProvider::Tavily),
                    Some(SearchProvider::Tavily) => Some(SearchProvider::Exa),
                    Some(SearchProvider::Exa) => Some(SearchProvider::Linkup),
                    Some(SearchProvider::Linkup) => Some(SearchProvider::Perplexity),
                    Some(SearchProvider::Perplexity) | Some(SearchProvider::GitHub) => None,
                };
            }
            SettingsField::WorkflowScope => {
                self.workflow_scope = match self.workflow_scope {
                    WorkflowScopePreference::Project => WorkflowScopePreference::Root,
                    WorkflowScopePreference::Root => WorkflowScopePreference::Project,
                };
            }
            SettingsField::WorkflowAutoCommit => {
                self.workflow_auto_commit = !self.workflow_auto_commit;
            }
            SettingsField::WorkflowAutoCloseParent => {
                self.workflow_auto_close_parent = !self.workflow_auto_close_parent;
            }
            SettingsField::WorkflowVerifyTimeout => {
                self.workflow_verify_timeout =
                    self.workflow_verify_timeout.saturating_add(30).min(3600);
            }
            SettingsField::WorkflowRunBackground => {
                self.workflow_run_background = !self.workflow_run_background;
            }
            SettingsField::WorkflowMaxWorkers => {
                self.workflow_max_workers = self.workflow_max_workers.saturating_add(1).min(32);
            }
            SettingsField::WorkflowReviewAfterRun => {
                self.workflow_review_after_run = !self.workflow_review_after_run;
            }
            SettingsField::WorkflowContinueAfterFailure => {
                self.workflow_continue_after_failure = !self.workflow_continue_after_failure;
            }
            SettingsField::TavilyApiKey => {}
            SettingsField::ExaApiKey => {}
            SettingsField::Save => {}
        }
    }

    /// Cycle the current field's value backward.
    pub fn cycle_backward(&mut self) {
        self.dirty = true;
        match self.current_field() {
            SettingsField::Model => {
                if !self.model_options.is_empty() {
                    if let Some(idx) = self.model_options.iter().position(|m| *m == self.model) {
                        let prev = if idx == 0 {
                            self.model_options.len() - 1
                        } else {
                            idx - 1
                        };
                        self.model = self.model_options[prev].clone();
                    }
                }
            }
            SettingsField::ChosenModels => {
                self.toggle_current_model_in_chosen();
            }
            SettingsField::Theme => {
                if !self.theme_options.is_empty() {
                    let idx = self
                        .theme_options
                        .iter()
                        .position(|t| *t == self.theme_name)
                        .unwrap_or(0);
                    let prev = if idx == 0 {
                        self.theme_options.len() - 1
                    } else {
                        idx - 1
                    };
                    self.theme_name = self.theme_options[prev].clone();
                }
            }
            SettingsField::ThinkingLevel => {
                self.thinking_level = prev_thinking(self.thinking_level);
            }
            SettingsField::MaxTokens => {
                self.max_tokens = self.max_tokens.saturating_sub(256).max(1);
            }
            SettingsField::MaxTurns => {
                self.max_turns = self.max_turns.saturating_sub(10).max(1);
            }
            SettingsField::ObservationMask => {
                self.observation_mask = (self.observation_mask - 0.05).max(0.0);
            }
            SettingsField::ReadMaxLines => {
                self.read_max_lines = self.read_max_lines.saturating_sub(100);
            }
            SettingsField::SidebarWidth => {
                self.sidebar_width = self.sidebar_width.saturating_sub(5).max(20);
            }
            SettingsField::WordWrap => {
                self.word_wrap = !self.word_wrap;
            }
            SettingsField::Animations => {
                self.animations = match self.animations {
                    AnimationLevel::None => AnimationLevel::Minimal,
                    AnimationLevel::Spinner => AnimationLevel::None,
                    AnimationLevel::Minimal => AnimationLevel::Spinner,
                };
            }
            SettingsField::AutoOpenSidebar => {
                self.auto_open_sidebar = !self.auto_open_sidebar;
            }
            SettingsField::SidebarAutoOpenWidth => {
                self.sidebar_auto_open_width =
                    self.sidebar_auto_open_width.saturating_sub(10).max(40);
            }
            SettingsField::ThinkingLines => {
                self.thinking_lines = self.thinking_lines.saturating_sub(1).max(1);
            }
            SettingsField::StreamingLines => {
                self.streaming_lines = self.streaming_lines.saturating_sub(1).max(1);
            }
            SettingsField::MouseScrollLines => {
                self.mouse_scroll_lines = self.mouse_scroll_lines.saturating_sub(1).max(1);
            }
            SettingsField::KeyboardScrollLines => {
                self.keyboard_scroll_lines = self.keyboard_scroll_lines.saturating_sub(5).max(5);
            }
            SettingsField::ShowTimestamps => {
                self.show_timestamps = !self.show_timestamps;
            }
            SettingsField::ShowCost => {
                self.show_cost = !self.show_cost;
            }
            SettingsField::ShowContextUsage => {
                self.show_context_usage = !self.show_context_usage;
            }
            SettingsField::NotifyOnAgentComplete => {
                self.notify_on_agent_complete = !self.notify_on_agent_complete;
            }
            SettingsField::ContinuePolicy => {
                self.continue_policy = match self.continue_policy {
                    ContinuePolicy::Disabled => ContinuePolicy::Aggressive,
                    ContinuePolicy::Conservative => ContinuePolicy::Disabled,
                    ContinuePolicy::Balanced => ContinuePolicy::Conservative,
                    ContinuePolicy::Aggressive => ContinuePolicy::Balanced,
                };
            }
            SettingsField::AgentMode => {
                self.agent_mode = match self.agent_mode {
                    AgentMode::Full => AgentMode::Auditor,
                    AgentMode::Worker => AgentMode::Full,
                    AgentMode::Orchestrator => AgentMode::Worker,
                    AgentMode::Planner => AgentMode::Orchestrator,
                    AgentMode::Reviewer => AgentMode::Planner,
                    AgentMode::Auditor => AgentMode::Reviewer,
                };
            }
            SettingsField::WriteOverwritePolicy => {
                self.write_overwrite_policy = match self.write_overwrite_policy {
                    WriteOverwritePolicy::Warn => WriteOverwritePolicy::Deny,
                    WriteOverwritePolicy::RequireRead => WriteOverwritePolicy::Warn,
                    WriteOverwritePolicy::BlockStale => WriteOverwritePolicy::RequireRead,
                    WriteOverwritePolicy::Deny => WriteOverwritePolicy::BlockStale,
                };
            }
            SettingsField::ShellBackend => {
                self.shell_backend = match self.shell_backend {
                    ShellBackend::Sh => ShellBackend::RushDaemon,
                    ShellBackend::Rush => ShellBackend::Sh,
                    ShellBackend::RushDaemon => ShellBackend::Rush,
                };
            }
            SettingsField::LuaNativeTools => self.lua_native_tools = !self.lua_native_tools,
            SettingsField::LuaShellExec => self.lua_shell_exec = !self.lua_shell_exec,
            SettingsField::LuaHttp => self.lua_http = !self.lua_http,
            SettingsField::LuaSecrets => self.lua_secrets = !self.lua_secrets,
            SettingsField::ImproveAutoTurnBudget => {
                self.improve_auto_turn_budget =
                    self.improve_auto_turn_budget.saturating_sub(1).max(1);
            }
            SettingsField::LoopTurnBudget => {
                self.loop_turn_budget = self.loop_turn_budget.saturating_sub(1);
            }
            SettingsField::WebSearchProvider => {
                self.web_search_provider = match self.web_search_provider {
                    None => Some(SearchProvider::Perplexity),
                    Some(SearchProvider::Tavily) | Some(SearchProvider::GitHub) => None,
                    Some(SearchProvider::Exa) => Some(SearchProvider::Tavily),
                    Some(SearchProvider::Linkup) => Some(SearchProvider::Exa),
                    Some(SearchProvider::Perplexity) => Some(SearchProvider::Linkup),
                };
            }
            SettingsField::WorkflowScope => {
                self.workflow_scope = match self.workflow_scope {
                    WorkflowScopePreference::Project => WorkflowScopePreference::Root,
                    WorkflowScopePreference::Root => WorkflowScopePreference::Project,
                };
            }
            SettingsField::WorkflowAutoCommit => {
                self.workflow_auto_commit = !self.workflow_auto_commit;
            }
            SettingsField::WorkflowAutoCloseParent => {
                self.workflow_auto_close_parent = !self.workflow_auto_close_parent;
            }
            SettingsField::WorkflowVerifyTimeout => {
                self.workflow_verify_timeout = self.workflow_verify_timeout.saturating_sub(30);
            }
            SettingsField::WorkflowRunBackground => {
                self.workflow_run_background = !self.workflow_run_background;
            }
            SettingsField::WorkflowMaxWorkers => {
                self.workflow_max_workers = self.workflow_max_workers.saturating_sub(1).max(1);
            }
            SettingsField::WorkflowReviewAfterRun => {
                self.workflow_review_after_run = !self.workflow_review_after_run;
            }
            SettingsField::WorkflowContinueAfterFailure => {
                self.workflow_continue_after_failure = !self.workflow_continue_after_failure;
            }
            SettingsField::TavilyApiKey => {}
            SettingsField::ExaApiKey => {}
            SettingsField::Save => {}
        }
    }

    /// Begin direct numeric input for the current field.
    pub fn start_edit(&mut self) {
        match self.current_field() {
            SettingsField::MaxTokens => {
                self.editing_number = true;
                self.edit_buffer = self.max_tokens.to_string();
            }
            SettingsField::MaxTurns => {
                self.editing_number = true;
                self.edit_buffer = self.max_turns.to_string();
            }
            SettingsField::ImproveAutoTurnBudget => {
                self.editing_number = true;
                self.edit_buffer = self.improve_auto_turn_budget.to_string();
            }
            SettingsField::LoopTurnBudget => {
                self.editing_number = true;
                self.edit_buffer = self.loop_turn_budget.to_string();
            }
            SettingsField::ObservationMask => {
                self.editing_number = true;
                self.edit_buffer = format!("{:.2}", self.observation_mask);
            }
            SettingsField::ReadMaxLines => {
                self.editing_number = true;
                self.edit_buffer = self.read_max_lines.to_string();
            }
            SettingsField::WorkflowVerifyTimeout => {
                self.editing_number = true;
                self.edit_buffer = self.workflow_verify_timeout.to_string();
            }
            SettingsField::WorkflowMaxWorkers => {
                self.editing_number = true;
                self.edit_buffer = self.workflow_max_workers.to_string();
            }
            SettingsField::SidebarWidth => {
                self.editing_number = true;
                self.edit_buffer = self.sidebar_width.to_string();
            }
            SettingsField::TavilyApiKey => {
                self.editing_number = false;
                self.edit_buffer = self.tavily_api_key.clone();
            }
            SettingsField::ExaApiKey => {
                self.editing_number = false;
                self.edit_buffer = self.exa_api_key.clone();
            }
            _ => {
                // For enum/bool fields, Enter cycles forward
                self.cycle_forward();
            }
        }
    }

    pub fn push_char(&mut self, c: char) {
        if self.editing_number {
            if c.is_ascii_digit() || c == '.' {
                self.edit_buffer.push(c);
            }
            return;
        }

        match self.current_field() {
            SettingsField::TavilyApiKey => {
                self.tavily_api_key.push(c);
                self.dirty = true;
            }
            SettingsField::ExaApiKey => {
                self.exa_api_key.push(c);
                self.dirty = true;
            }
            SettingsField::ChosenModels => {
                if !c.is_control() {
                    let lower = c.to_ascii_lowercase();
                    if let Some(next) = self
                        .model_options
                        .iter()
                        .find(|m| m.to_ascii_lowercase().starts_with(lower))
                    {
                        self.model = next.clone();
                    }
                }
            }
            _ => {}
        }
    }

    pub fn pop_char(&mut self) {
        if self.editing_number {
            self.edit_buffer.pop();
            return;
        }

        match self.current_field() {
            SettingsField::TavilyApiKey => {
                self.tavily_api_key.pop();
                self.dirty = true;
            }
            SettingsField::ExaApiKey => {
                self.exa_api_key.pop();
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Commit the edit buffer to the underlying field value.
    pub fn commit_edit(&mut self) {
        if !self.editing_number {
            return;
        }
        self.editing_number = false;
        self.dirty = true;
        match self.current_field() {
            SettingsField::MaxTokens => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.max_tokens = v.max(1);
                }
            }
            SettingsField::MaxTurns => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.max_turns = v.max(1);
                }
            }
            SettingsField::ImproveAutoTurnBudget => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.improve_auto_turn_budget = v.clamp(1, 100);
                }
            }
            SettingsField::LoopTurnBudget => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.loop_turn_budget = v.min(100);
                }
            }
            SettingsField::ObservationMask => {
                if let Ok(v) = self.edit_buffer.parse::<f64>() {
                    self.observation_mask = v.clamp(0.0, 1.0);
                }
            }
            SettingsField::ReadMaxLines => {
                if let Ok(v) = self.edit_buffer.parse::<usize>() {
                    self.read_max_lines = v;
                }
            }
            SettingsField::WorkflowVerifyTimeout => {
                if let Ok(v) = self.edit_buffer.parse::<u64>() {
                    self.workflow_verify_timeout = v.min(3600);
                }
            }
            SettingsField::WorkflowMaxWorkers => {
                if let Ok(v) = self.edit_buffer.parse::<u32>() {
                    self.workflow_max_workers = v.clamp(1, 32);
                }
            }
            SettingsField::SidebarWidth => {
                if let Ok(v) = self.edit_buffer.parse::<u16>() {
                    self.sidebar_width = v.clamp(20, 80);
                }
            }
            _ => {}
        }
        self.edit_buffer.clear();
    }

    /// Write current settings into a Config for saving and in-session use.
    pub fn apply_to_config(&self, config: &mut Config) {
        config.model = Some(self.model.clone());
        config.enabled_models = if self.chosen_models.is_empty() {
            None
        } else {
            Some(self.chosen_models.clone())
        };
        config.theme = Some(self.theme_name.clone());
        config.thinking = Some(self.thinking_level);
        config.max_tokens = Some(self.max_tokens);
        config.max_turns = Some(self.max_turns);
        config.context = ContextConfig {
            observation_mask_threshold: self.observation_mask,
            ..config.context.clone()
        };
        config.ui = imp_core::config::UiConfig {
            sidebar_style: SidebarStyle::Inspector,
            tool_output: ToolOutputDisplay::Full,
            tool_output_lines: self.tool_output_lines,
            read_max_lines: self.read_max_lines,
            sidebar_width: self.sidebar_width,
            word_wrap: self.word_wrap,
            animations: self.animations,
            hide_tools_in_chat: false,
            chat_tool_display: ChatToolDisplay::Summary,
            auto_open_sidebar: self.auto_open_sidebar,
            sidebar_auto_open_width: self.sidebar_auto_open_width,
            thinking_lines: self.thinking_lines,
            streaming_lines: self.streaming_lines,
            mouse_scroll_lines: self.mouse_scroll_lines,
            keyboard_scroll_lines: self.keyboard_scroll_lines,
            mouse_capture: config.ui.mouse_capture,
            show_timestamps: self.show_timestamps,
            show_cost: self.show_cost,
            show_context_usage: self.show_context_usage,
            notify_on_agent_complete: self.notify_on_agent_complete,
            continue_policy: self.continue_policy,
            build_auto_turn_budget: config.ui.build_auto_turn_budget,
            improve_auto_turn_budget: self.improve_auto_turn_budget,
            loop_turn_budget: self.loop_turn_budget,
        };
        config.web = imp_core::tools::web::types::WebConfig {
            search_provider: self.web_search_provider,
        };
        config.mode = self.agent_mode;
        config.write.overwrite_policy = self.write_overwrite_policy;
        config.shell.backend = self.shell_backend.clone();
        config.lua = LuaConfig {
            allow_native_tool_calls: Some(self.lua_native_tools),
            allow_shell_exec: Some(self.lua_shell_exec),
            allow_http: Some(self.lua_http),
            allow_secrets: Some(self.lua_secrets),
            allowed_env: config.lua.allowed_env.clone(),
        };
        config.workflow = WorkflowConfig {
            scope: self.workflow_scope,
            auto_commit: self.workflow_auto_commit,
            auto_close_parent: self.workflow_auto_close_parent,
            verify_timeout: (self.workflow_verify_timeout > 0)
                .then_some(self.workflow_verify_timeout),
            run: WorkflowRunConfig {
                background: self.workflow_run_background,
                max_workers: self.workflow_max_workers.max(1),
                continue_after_failure: self.workflow_continue_after_failure,
                review_after_run: self.workflow_review_after_run,
            },
        };
    }
    fn model_is_chosen(&self, model_id: &str) -> bool {
        self.chosen_models.iter().any(|m| m == model_id)
    }

    fn toggle_current_model_in_chosen(&mut self) {
        let model = self.model.clone();
        if let Some(idx) = self.chosen_models.iter().position(|m| m == &model) {
            self.chosen_models.remove(idx);
        } else {
            self.chosen_models.push(model);
        }
    }

    fn chosen_models_summary(&self) -> String {
        if self.chosen_models.is_empty() {
            "all models".to_string()
        } else {
            format!("{} chosen", self.chosen_models.len())
        }
    }
}

pub use render::SettingsView;

#[cfg(test)]
#[path = "settings/tests.rs"]
mod tests;
