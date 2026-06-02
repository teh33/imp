mod agent_events;
mod agent_start;
mod ask;
mod commands;
mod compaction;
mod diagnostics;
mod event_kinds;
mod event_loop;
mod git_status;
mod helpers;
mod input;
mod lifecycle;
mod messages;
mod model_auth;
mod models;
mod overlay;
mod rendering;
mod runtime_signals;
mod sessions;
mod settings_flow;
mod startup_surface;
mod state_types;
mod tools;
mod verification_status;
mod welcome_flow;
mod workflow_run_summary;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_start::{
    agent_start_join_result_to_signal, start_agent_from_request, AgentStartRequest,
    AgentStartResult,
};
use diagnostics::{
    extension_policy_warning, open_path_in_editor, provenance_warning,
    selected_read_file_path_from_tool, trace_tui_to, trust_policy_warning, TuiTrace,
};
use event_kinds::{agent_event_kind, runtime_signal_kind};
use git_status::compact_git_label;
use helpers::{
    bump_epoch, command_option_value, lua_result_requests_restart, parse_secret_field_names,
    single_line_preview, stable_hash, strip_lua_restart_directive,
};
use imp_core::eval_candidate::{
    redact_eval_candidate, EvalActualBehavior, EvalCandidate, EvalExpectedBehavior,
    EvalFailureMode, EvalPrivacy, EvalRedactionStatus, EvalVerifier,
};
use imp_core::format_error_for_display;
use imp_core::ui::WidgetContent;
use imp_core::WorkflowUnitRef;
use model_auth::{
    filtered_model_options, include_current_model_option, oauth_provider, provider_logged_in,
    resolve_provider_api_key, should_use_chatgpt_provider,
};
use startup_surface::{
    discover_rule_files, discover_startup_workflows, repo_stats_label, repo_stats_scan_root,
    rule_file_lines, startup_skill_detail_render_data, startup_skill_hits,
    startup_workflow_detail_render_data, startup_workflow_hits, strip_status_suffix,
    workflow_startup_lines, RepoStatsScanRoot,
};
use state_types::{
    ChatRenderCache, ChatRenderCacheKey, DragAutoScroll, RepoStatsState, ScrollDirection,
    SidebarDetailCache, SidebarDetailCacheKey, SidebarStreamCache, SidebarStreamCacheKey,
    StartupSkillHit, StartupSurfaceData, StartupSurfaceMetadata, StartupWorkflowHit,
    StartupWorkflowItem, ThemeKind,
};
use verification_status::{verification_gate_label, verification_status_text};
use workflow_run_summary::workflow_run_detail_render_data;
#[cfg(feature = "mana-ui")]
use workflow_run_summary::workflow_run_summary_cache_key;
#[cfg(not(feature = "mana-ui"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowRunSummary {
    run_id: String,
    scope: String,
    status: String,
    total_units: u32,
    total_closed: u32,
    total_failed: u32,
    total_awaiting_verify: u32,
    latest: Option<String>,
    logs: Vec<String>,
    agents: Vec<WorkflowRunAgentSummary>,
}
#[cfg(not(feature = "mana-ui"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowRunAgentSummary {
    unit_id: String,
    status: String,
    action: String,
    title: String,
}
#[allow(dead_code)]
fn workflow_run_summary(_id: &str) -> Result<Option<WorkflowRunSummary>, String> {
    Ok(None)
}
fn stop_workflow_run(_id: &str) -> Result<Option<WorkflowRunSummary>, String> {
    Ok(None)
}

use imp_lua::LuaRuntime;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use imp_core::agent::{AgentCommand, AgentEvent, AgentHandle};
use imp_core::config::Config;
use imp_core::runtime::{RuntimeStateAccumulator, RuntimeStateSnapshot};
use imp_core::session::{SessionEntry, SessionInfo, SessionManager};
use imp_core::tools::ToolRegistry;
use imp_core::trust::TrustLabel;
use imp_core::Error as ImpCoreError;
use imp_llm::auth::AuthStore;
use imp_llm::model::{ModelMeta, ModelRegistry, ProviderRegistry};
use imp_llm::{Cost, ThinkingLevel, Usage};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::animation::{
    title_loop_frame, title_loop_glyph, title_spinner_frame, title_working_glyph,
};
use crate::highlight::Highlighter;
use crate::selection::{
    extract_selected_text, SelectablePane, SelectionOverlay, SelectionState, TextSurface,
};
use crate::terminal::{ring_terminal_bell, InteractiveTerminal};
use crate::theme::Theme;
use crate::turn_tracker::TurnTracker;
use crate::views::chat::{DisplayMessage, MessageRole, RenderedChatView};
use crate::views::command_palette::{
    builtin_commands, merge_extension_commands, merge_skill_commands, CommandPaletteState,
    CommandPaletteView,
};
use crate::views::editor::{EditorState, WorkflowMode};
use crate::views::login_picker::{login_providers, LoginPickerState, LoginPickerView};
#[cfg(feature = "mana-ui")]
use crate::views::mana_navigator::{ManaNavigatorState, ManaNavigatorView};
use crate::views::model_selector::{ModelSelectorState, ModelSelectorView};
use crate::views::secrets_picker::{secret_providers, SecretsPickerState, SecretsPickerView};
use crate::views::session_picker::{SessionPickerState, SessionPickerView};
use crate::views::settings::{SettingsState, SettingsView};
use crate::views::sidebar::{
    build_detail_text_surface_from_plain_lines, sidebar_sub_areas, thinking_detail_render_data,
    Sidebar, SidebarDetailRenderData, SidebarView,
};
use crate::views::startup::{StartupAction, StartupPanelView};
use crate::views::tools::{tool_display_icon, tool_display_name, DisplayToolCall};
use crate::views::tree::{flatten_tree, TreeView, TreeViewState};
use crate::views::welcome::{needs_welcome, WelcomeState, WelcomeStep, WelcomeView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Chat,
    SidebarList,
    SidebarDetail,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum UiMode {
    Normal,
    ModelSelector(ModelSelectorState),
    CommandPalette(CommandPaletteState),
    LoginPicker(LoginPickerState),
    #[cfg(feature = "mana-ui")]
    ManaNavigator(ManaNavigatorState),
    SecretsPicker(SecretsPickerState),
    TreeView(TreeViewState),
    Settings(SettingsState),
    SessionPicker(SessionPickerState),
    Welcome(WelcomeState),
}

#[derive(Debug, Clone)]
pub enum QueuedMessage {
    Steer(String),
    FollowUp(String),
}

impl QueuedMessage {
    fn text(&self) -> &str {
        match self {
            QueuedMessage::Steer(text) | QueuedMessage::FollowUp(text) => text,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum AskReply {
    Select(tokio::sync::oneshot::Sender<Option<usize>>),
    SelectOrInput(tokio::sync::oneshot::Sender<Option<imp_core::ui::SelectionAnswer>>),
    MultiSelect(tokio::sync::oneshot::Sender<Option<Vec<usize>>>),
    Input(tokio::sync::oneshot::Sender<Option<String>>),
}

#[derive(Debug)]
enum LoginTaskExit {
    Success(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeAuthMethod {
    OAuth,
    ApiKey,
}

struct SessionOpenResult {
    session: SessionManager,
    summary: Option<String>,
}

impl std::fmt::Debug for SessionOpenResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionOpenResult")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

const SESSION_LIST_PAGE_SIZE: usize = 24;
const SESSION_LIST_PREFETCH_REMAINING: usize = 6;

#[derive(Debug)]
struct SessionListResult {
    sessions: Vec<SessionInfo>,
    preferred_cwd: PathBuf,
    offset: usize,
    limit: usize,
}

fn open_url(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open").arg(url).spawn().is_ok();
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("BROWSER").is_none()
        {
            return false;
        }
        return std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .is_ok();
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()
            .is_ok();
    }
    #[allow(unreachable_code)]
    false
}

fn search_provider_docs_url(provider: &str) -> &'static str {
    match provider {
        "tavily" => "https://app.tavily.com/home",
        "exa" => "https://dashboard.exa.ai/api-keys",
        "linkup" => "https://app.linkup.so/api-keys",
        "perplexity" => "https://www.perplexity.ai/settings/api",
        _ => "",
    }
}

fn prompt_text_for_secret_provider(provider: &str) -> String {
    let docs = search_provider_docs_url(provider);
    let mut lines = vec![format!("Configure secure credentials for {provider}")];
    if !docs.is_empty() {
        lines.push(String::new());
        lines.push(format!("Get credentials at: {docs}"));
    }
    lines.push(String::new());
    lines.push("First enter a comma-separated field list (default: api_key).".into());
    lines.push("Then imp will prompt for each field value.".into());
    lines.join("\n")
}

#[derive(Debug)]
enum SecretsFlowState {
    AwaitingFieldNames {
        provider: String,
    },
    AwaitingFieldValues {
        provider: String,
        fields: Vec<String>,
        current: usize,
        values: HashMap<String, String>,
    },
}

const MAX_RUNTIME_SIGNALS_PER_TICK: usize = 256;
const MAX_UI_REQUESTS_PER_TICK: usize = 16;
const MAX_TERMINAL_EVENTS_PER_TICK: usize = 32;
const MAX_RUNTIME_SIGNAL_BATCH: usize = 256;
const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_FRAME_INTERVAL: Duration = Duration::from_millis(100);
const SLOW_TUI_EVENT_THRESHOLD: Duration = Duration::from_millis(16);
const SLOW_TUI_RENDER_THRESHOLD: Duration = Duration::from_millis(33);
const AGENT_START_STATUS_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum RuntimeSignal {
    AgentEvent(AgentEvent),
    AgentTaskCompleted,
    AgentTaskFailed(String),
    CompactionTaskCompleted(String),
    CompactionTaskFailed(String),
    LuaCommandCompleted {
        command: String,
        result: Option<String>,
    },
    LuaCommandRestartRequested {
        command: String,
        result: Option<String>,
    },
    LuaCommandFailed {
        command: String,
        error: String,
    },
    LoginUrlReady {
        provider: String,
        url: String,
        browser_opened: bool,
    },
    LoginTaskSucceeded(String),
    LoginTaskFailed(String),
    SessionListLoaded(SessionListResult),
    SessionListFailed(String),
    SessionOpened(SessionOpenResult),
    SessionOpenFailed(String),
    UserMessagePersisted {
        entry_id: String,
        persisted_session: Option<SessionManager>,
    },
    UserMessagePersistFailed(String),
    AgentStartCompleted(AgentStartResult),
    AgentStartFailed(String),
    AgentStartStatus {
        key: String,
        text: Option<String>,
    },
    #[cfg(feature = "mana-ui")]
    ManaNavigatorLoaded(ManaNavigatorState),
    #[cfg(feature = "mana-ui")]
    ManaNavigatorLoadFailed {
        mana_dir: Option<PathBuf>,
        message: String,
    },
    RepoStatsLoaded(Result<Option<crate::repo_stats::RepoStats>, String>),
    RepoStatsSkipped(RepoStatsState),
    UiRequest(crate::tui_interface::UiRequest),
}

#[derive(Debug, Clone)]
struct LoopState {
    message: String,
    completed_turns: u32,
    budget: Option<u32>,
}

#[derive(Debug, Clone)]
struct GitLabelCache {
    cwd: PathBuf,
    refreshed_at: Instant,
    label: Option<String>,
}

#[derive(Debug)]
struct StartupSkillDetailCache {
    skill_path: PathBuf,
    theme: ThemeKind,
    render: SidebarDetailRenderData,
}

type LuaCommandTask = tokio::task::JoinHandle<(String, Result<Option<String>, String>)>;

pub struct App {
    // Core
    pub running: bool,
    pub messages: Vec<DisplayMessage>,
    pub editor: EditorState,
    ask_editor_backup: Option<EditorState>,
    pub cwd: PathBuf,

    // Agent
    pub agent_handle: Option<AgentHandle>,
    agent_event_task: Option<tokio::task::JoinHandle<()>>,
    agent_task: Option<tokio::task::JoinHandle<Result<(), ImpCoreError>>>,
    agent_start_task: Option<tokio::task::JoinHandle<()>>,
    compaction_task: Option<tokio::task::JoinHandle<Result<String, String>>>,
    lua_command_task: Option<LuaCommandTask>,
    pub is_streaming: bool,
    pub message_queue: Vec<QueuedMessage>,
    pending_agent_prompt: Option<String>,
    pending_agent_visible_text: Option<String>,
    pending_agent_cwd: Option<PathBuf>,

    // Session
    pub session: SessionManager,

    // Config
    pub config: Config,
    pub model_name: String,
    pub role_name: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub context_window: u32,

    // UI state
    pub mode: UiMode,
    pub scroll_offset: usize,
    streaming_anchor_user_index: Option<usize>,
    pub auto_scroll: bool,
    pub tools_expanded: bool,
    /// Index into the flattened tool call list. `None` means inspector follows latest.
    pub tool_focus: Option<usize>,
    /// True once the user explicitly selects a tool; prevents new tools stealing focus.
    pub tool_focus_pinned: bool,
    /// True while inspector should keep live output pinned to the bottom.
    pub sidebar_auto_follow: bool,

    pub ctrl_c_count: u8,
    pub needs_redraw: bool,
    last_terminal_title: Option<String>,
    pub last_esc: Option<Instant>,
    pub tick: u64,
    completed_turns_in_run: u32,
    last_agent_error: Option<String>,
    suppress_completion_notification: bool,
    pub ui_rx: Option<tokio::sync::mpsc::Receiver<crate::tui_interface::UiRequest>>,
    lua_command_ui: Option<Arc<dyn imp_core::ui::UserInterface>>,
    pub ask_state: Option<crate::views::ask_bar::AskState>,
    pub ask_reply: Option<AskReply>,
    pub workflow_mode: WorkflowMode,
    active_workflow_scope: Option<WorkflowUnitRef>,
    active_workflow_run: Option<WorkflowRunSummary>,
    loop_state: Option<LoopState>,
    secrets_flow: Option<SecretsFlowState>,
    login_task: Option<tokio::task::JoinHandle<LoginTaskExit>>,
    session_list_task: Option<tokio::task::JoinHandle<()>>,
    session_open_task: Option<tokio::task::JoinHandle<()>>,
    user_message_persist_task: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "mana-ui")]
    mana_navigator_task: Option<tokio::task::JoinHandle<()>>,
    runtime_signal_tx: tokio::sync::mpsc::UnboundedSender<RuntimeSignal>,
    runtime_signal_rx: tokio::sync::mpsc::UnboundedReceiver<RuntimeSignal>,
    tui_trace: Option<TuiTrace>,

    // Accumulated stats
    pub accumulated_usage: Usage,
    pub accumulated_cost: Cost,
    /// Last turn's input tokens — best proxy for actual current context size.
    pub current_context_tokens: u32,
    chat_render_epoch: u64,

    current_oauth_display_info: Option<imp_llm::auth::OAuthDisplayInfo>,
    current_oauth_display_info_model: String,
    current_model_meta_for_persistence: Option<ModelMeta>,
    current_model_meta_for_persistence_model: String,
    git_label_cache: Option<GitLabelCache>,
    startup_skill_detail_cache: Option<StartupSkillDetailCache>,
    startup_surface_metadata: StartupSurfaceMetadata,

    // Extension state
    pub status_items: HashMap<String, String>,
    verification_status_items: BTreeMap<String, String>,
    runtime_state: RuntimeStateAccumulator,
    runtime_event_sequence: u64,
    pub runtime_snapshot: RuntimeStateSnapshot,
    pub widgets: HashMap<String, WidgetContent>,

    /// Lua extension runtime (for command dispatch and hot-reload).
    pub lua_runtime: Option<Arc<Mutex<LuaRuntime>>>,

    /// Startup skill selected for display in the inspector sidebar.
    selected_startup_skill: Option<imp_core::resources::Skill>,
    selected_startup_workflow: Option<StartupWorkflowItem>,

    // Sidebar
    pub sidebar: Sidebar,

    /// Which pane has focus for scroll routing.
    pub active_pane: Pane,
    /// Sidebar list area cached from last render (for click/scroll detection).
    pub sidebar_list_rect: Option<Rect>,
    /// Sidebar detail area cached from last render (for click/scroll detection).
    pub sidebar_detail_rect: Option<Rect>,
    /// Cached selectable chat surface from last render.
    pub chat_surface: Option<TextSurface>,
    /// Cached tool header hit map from last chat render.
    chat_tool_click_map: Vec<(u16, String)>,
    /// Cached selectable sidebar detail surface from last render.
    pub sidebar_detail_surface: Option<TextSurface>,
    /// Current app-native text selection.
    pub selection: Option<SelectionState>,
    /// Selection anchor while dragging with the mouse.
    pub drag_selection: Option<SelectablePane>,
    /// Active edge-autoscroll while dragging a selection.
    drag_autoscroll: Option<DragAutoScroll>,
    /// Cached chat render data reused while only scroll offset changes.
    chat_render_cache: Option<ChatRenderCache>,
    sidebar_stream_cache: Option<SidebarStreamCache>,
    sidebar_detail_cache: Option<SidebarDetailCache>,

    // Turn activity tracking
    llm_thought_segment_started_at: Option<Instant>,
    pub turn_tracker: TurnTracker,
    agent_turn_started_at: Option<Instant>,
    first_agent_event_seen: bool,

    // Display helpers
    pub theme: Theme,
    pub highlighter: Highlighter,
    pub model_registry: ModelRegistry,
}

impl App {
    pub fn new(
        config: Config,
        session: SessionManager,
        model_registry: ModelRegistry,
        cwd: PathBuf,
    ) -> Self {
        let model_name = config.model.clone().unwrap_or_else(|| "sonnet".into());
        let thinking_level = config.thinking.unwrap_or(ThinkingLevel::Medium);
        let theme = Theme::named(config.theme.as_deref().unwrap_or("default"));
        let context_window = model_registry
            .resolve_meta(&model_name, None)
            .map(|m| m.context_window)
            .unwrap_or(200_000);
        let (runtime_signal_tx, runtime_signal_rx) = tokio::sync::mpsc::unbounded_channel();
        let startup_surface_metadata =
            Self::load_startup_surface_metadata(&cwd, &config, &model_registry, &model_name);
        let repo_stats_tx = runtime_signal_tx.clone();
        let repo_stats_cwd = cwd.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                match repo_stats_scan_root(&repo_stats_cwd) {
                    RepoStatsScanRoot::Scan(root) => {
                        let result = tokio::task::spawn_blocking(move || {
                            crate::repo_stats::scan_repo(&root)
                        })
                        .await
                        .map_err(|error| format!("repo stats task failed: {error}"))
                        .and_then(|result| result.map_err(|error| error.to_string()));
                        let _ = repo_stats_tx.send(RuntimeSignal::RepoStatsLoaded(result));
                    }
                    RepoStatsScanRoot::Skip(state) => {
                        let _ = repo_stats_tx.send(RuntimeSignal::RepoStatsSkipped(state));
                    }
                }
            });
        }

        Self {
            running: true,
            messages: Vec::new(),
            editor: EditorState::new(),
            ask_editor_backup: None,
            cwd,
            workflow_mode: WorkflowMode::Normal,
            agent_handle: None,
            agent_event_task: None,
            agent_task: None,
            agent_start_task: None,
            compaction_task: None,
            lua_command_task: None,
            is_streaming: false,
            message_queue: Vec::new(),
            pending_agent_prompt: None,
            pending_agent_visible_text: None,
            pending_agent_cwd: None,
            session,
            config,
            model_name,
            role_name: None,
            thinking_level,
            context_window,
            mode: UiMode::Normal,
            scroll_offset: 0,
            streaming_anchor_user_index: None,
            auto_scroll: true,
            tools_expanded: false,
            tool_focus: None,
            tool_focus_pinned: false,
            sidebar_auto_follow: true,

            ctrl_c_count: 0,
            needs_redraw: true,
            last_terminal_title: None,
            last_esc: None,
            tick: 0,
            completed_turns_in_run: 0,
            last_agent_error: None,
            suppress_completion_notification: false,
            ui_rx: None,
            lua_command_ui: None,
            ask_state: None,
            ask_reply: None,
            active_workflow_scope: None,
            active_workflow_run: None,
            loop_state: None,
            secrets_flow: None,
            login_task: None,
            session_list_task: None,
            session_open_task: None,
            user_message_persist_task: None,
            #[cfg(feature = "mana-ui")]
            mana_navigator_task: None,
            runtime_signal_tx,
            runtime_signal_rx,
            tui_trace: TuiTrace::from_env(),
            accumulated_usage: Usage::default(),
            accumulated_cost: Cost::default(),
            current_context_tokens: 0,
            chat_render_epoch: 0,
            current_oauth_display_info: None,
            current_oauth_display_info_model: String::new(),
            current_model_meta_for_persistence: None,
            current_model_meta_for_persistence_model: String::new(),
            git_label_cache: None,
            startup_skill_detail_cache: None,
            startup_surface_metadata,
            status_items: HashMap::new(),
            verification_status_items: BTreeMap::new(),
            runtime_state: RuntimeStateAccumulator::new("tui"),
            runtime_event_sequence: 0,
            runtime_snapshot: RuntimeStateSnapshot::default(),
            widgets: HashMap::new(),
            lua_runtime: None,
            selected_startup_skill: None,
            selected_startup_workflow: None,
            sidebar: Sidebar::default(),
            active_pane: Pane::Chat,
            sidebar_list_rect: None,
            sidebar_detail_rect: None,
            chat_surface: None,
            chat_tool_click_map: Vec::new(),
            sidebar_detail_surface: None,
            selection: None,
            drag_selection: None,
            drag_autoscroll: None,
            chat_render_cache: None,
            sidebar_stream_cache: None,
            sidebar_detail_cache: None,
            llm_thought_segment_started_at: None,
            turn_tracker: TurnTracker::new(),
            agent_turn_started_at: None,
            first_agent_event_seen: false,
            theme,
            highlighter: Highlighter::new(),
            model_registry,
        }
    }

    pub async fn run(
        &mut self,
        terminal: &mut InteractiveTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.prepare_for_interactive()?;
        self.event_loop(terminal).await
    }

    pub fn terminal_title(&self) -> String {
        let title = self
            .session
            .name()
            .map(str::to_string)
            .or_else(|| self.session.title(48))
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "chat".to_string());
        let identity = if self.loop_state.is_some() {
            if self.config.ui.animations == imp_core::config::AnimationLevel::None {
                title_loop_glyph()
            } else {
                title_loop_frame(self.tick)
            }
        } else if self.is_streaming
            || self.agent_start_task.is_some()
            || self.compaction_task.is_some()
        {
            if self.config.ui.animations == imp_core::config::AnimationLevel::None {
                title_working_glyph()
            } else {
                title_spinner_frame(self.tick)
            }
        } else {
            "imp"
        };
        format!("{identity} — {title}")
    }

    #[cfg(feature = "mana-ui")]
    fn open_mana_navigator(&mut self, initial_id: Option<&str>) {
        self.mode = UiMode::ManaNavigator(ManaNavigatorState::loading(&self.cwd));
        if self.mana_navigator_task.is_some() {
            return;
        }
        let cwd = self.cwd.clone();
        let initial_id = initial_id.map(str::to_string);
        let signal_tx = self.runtime_signal_tx.clone();
        self.mana_navigator_task = Some(tokio::spawn(async move {
            let signal = match tokio::task::spawn_blocking(move || {
                ManaNavigatorState::try_load(&cwd, initial_id.as_deref())
            })
            .await
            {
                Ok(Ok(state)) => RuntimeSignal::ManaNavigatorLoaded(state),
                Ok(Err((mana_dir, message))) => {
                    RuntimeSignal::ManaNavigatorLoadFailed { mana_dir, message }
                }
                Err(error) => RuntimeSignal::ManaNavigatorLoadFailed {
                    mana_dir: None,
                    message: format!("Mana navigator task failure: {error}"),
                },
            };
            let _ = signal_tx.send(signal);
        }));
    }

    #[cfg(feature = "mana-ui")]
    fn finish_mana_navigator_load(&mut self, state: ManaNavigatorState) {
        self.mana_navigator_task = None;
        if matches!(self.mode, UiMode::ManaNavigator(_)) {
            self.mode = UiMode::ManaNavigator(state);
        }
    }

    #[cfg(feature = "mana-ui")]
    fn fail_mana_navigator_load(&mut self, mana_dir: Option<PathBuf>, message: String) {
        self.mana_navigator_task = None;
        if matches!(self.mode, UiMode::ManaNavigator(_)) {
            self.mode = UiMode::ManaNavigator(ManaNavigatorState::error(mana_dir, message));
        } else {
            self.push_error_msg(&message);
        }
    }

    fn open_tree_view(&mut self) {
        let tree = self.session.get_tree();
        let flat = flatten_tree(&tree, 0);
        if flat.is_empty() {
            self.push_system_msg("No session history yet.");
            return;
        }
        let current_id = self.session.leaf_id().map(String::from);
        self.mode = UiMode::TreeView(TreeViewState::new(flat, current_id));
    }

    fn cycle_thinking_level(&mut self) {
        self.invalidate_chat_render_cache();
        self.thinking_level = match self.thinking_level {
            ThinkingLevel::Off => ThinkingLevel::Low,
            ThinkingLevel::Minimal => ThinkingLevel::Low,
            ThinkingLevel::Low => ThinkingLevel::Medium,
            ThinkingLevel::Medium => ThinkingLevel::High,
            ThinkingLevel::High => ThinkingLevel::XHigh,
            ThinkingLevel::XHigh => ThinkingLevel::Off,
        };
    }

    // ── Helpers ──────────────────────────────────────────────────
}

// ── Layout helpers ──────────────────────────────────────────────

/// Create a centered rect using percentage of the available area.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Check if a point is inside an optional rect.
fn point_in_rect(col: u16, row: u16, rect: Option<Rect>) -> bool {
    match rect {
        Some(r) => col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height,
        None => false,
    }
}

/// Create an area above the editor for a dropdown.
fn command_dropdown_area(editor_area: Rect, max_height: u16) -> Rect {
    let height = max_height.min(editor_area.y);
    Rect {
        x: editor_area.x,
        y: editor_area.y.saturating_sub(height),
        width: editor_area.width.min(60),
        height,
    }
}

fn command_arg(rest: &str) -> Option<&str> {
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(char::is_whitespace).map(str::trim)
    }
}

fn expand_prompt_path(path: &str, cwd: &Path) -> PathBuf {
    let expanded = if path == "~" {
        std::env::var_os("HOME").map(PathBuf::from)
    } else if let Some(rest) = path.strip_prefix("~/") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest))
    } else {
        None
    };

    let path = expanded.unwrap_or_else(|| PathBuf::from(path));
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod session_lifecycle;
