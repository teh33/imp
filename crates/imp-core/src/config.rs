use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use imp_llm::ThinkingLevel;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::guardrails::GuardrailConfig;
use crate::hooks::HookDef;
use crate::roles::{RoleDef, RoleRegistry, RoleRegistryError};
use crate::storage;
use crate::tools::browser::BrowserConfig;
use crate::tools::web::types::WebConfig;

/// Agent mode — controls which tools and workflow actions the agent may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMode {
    /// Default. Full access to all tools. No filtering.
    #[default]
    Full,
    /// Unit executor. Read + write + bash. No workflow create/run.
    Worker,
    /// Plans and executes via native workflows. Cannot touch files directly.
    Orchestrator,
    /// Decomposes work. Can read and update workflow plans. Cannot run them.
    Planner,
    /// Read-only inspector. No mutations, no workflows.
    Reviewer,
    /// Batch inspector. Reads code and workflow state, produces reports.
    Auditor,
}

const WORKER_TOOLS: &[&str] = &[
    "read", "scan", "web", "browser", "write", "edit", "bash", "git", "workflow", "ask_user",
];
const ORCHESTRATOR_TOOLS: &[&str] = &["read", "scan", "web", "workflow", "git", "ask_user"];
const PLANNER_TOOLS: &[&str] = &["read", "scan", "web", "git", "workflow", "ask_user"];
const REVIEWER_TOOLS: &[&str] = &["read", "scan", "web", "git", "ask_user"];
const AUDITOR_TOOLS: &[&str] = &["read", "scan", "web", "git", "workflow"];

const WORKER_WORKFLOW_ACTIONS: &[&str] = &["show", "update", "list", "validate"];
const ORCHESTRATOR_WORKFLOW_ACTIONS: &[&str] = &["list", "show", "validate", "run", "update"];
const PLANNER_WORKFLOW_ACTIONS: &[&str] = &["list", "show", "validate", "update"];
const AUDITOR_WORKFLOW_ACTIONS: &[&str] = &["list", "show", "validate"];

impl AgentMode {
    /// Tool names this mode permits. An empty slice means "allow all" (Full).
    pub fn allowed_tool_names(&self) -> &'static [&'static str] {
        match self {
            AgentMode::Full => &[],
            AgentMode::Worker => WORKER_TOOLS,
            AgentMode::Orchestrator => ORCHESTRATOR_TOOLS,
            AgentMode::Planner => PLANNER_TOOLS,
            AgentMode::Reviewer => REVIEWER_TOOLS,
            AgentMode::Auditor => AUDITOR_TOOLS,
        }
    }

    /// Returns true if the mode allows the named tool.
    pub fn allows_tool(&self, name: &str) -> bool {
        match self {
            AgentMode::Full => true,
            _ => self.allowed_tool_names().contains(&name),
        }
    }

    /// Workflow tool sub-actions this mode permits. An empty slice means "allow all" (Full).
    pub fn allowed_workflow_actions(&self) -> &'static [&'static str] {
        match self {
            AgentMode::Full | AgentMode::Reviewer => &[],
            AgentMode::Worker => WORKER_WORKFLOW_ACTIONS,
            AgentMode::Orchestrator => ORCHESTRATOR_WORKFLOW_ACTIONS,
            AgentMode::Planner => PLANNER_WORKFLOW_ACTIONS,
            AgentMode::Auditor => AUDITOR_WORKFLOW_ACTIONS,
        }
    }

    /// Returns true if the mode allows the named workflow action.
    pub fn allows_workflow_action(&self, action: &str) -> bool {
        match self {
            AgentMode::Full => true,
            AgentMode::Reviewer => false,
            _ => self.allowed_workflow_actions().contains(&action),
        }
    }

    /// Parse a mode from a string name (e.g. `"worker"`, `"full"`).
    ///
    /// Returns `None` for unrecognised names. Used to read `IMP_MODE` from the
    /// environment without requiring a full `FromStr` implementation.
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "full" => Some(AgentMode::Full),
            "worker" => Some(AgentMode::Worker),
            "orchestrator" => Some(AgentMode::Orchestrator),
            "planner" => Some(AgentMode::Planner),
            "reviewer" => Some(AgentMode::Reviewer),
            "auditor" => Some(AgentMode::Auditor),
            _ => None,
        }
    }

    /// Mode-specific behavioral instruction for the system prompt, if any.
    pub fn instructions(&self) -> Option<&'static str> {
        match self {
            AgentMode::Full => None,
            AgentMode::Worker => Some(
                "You are a worker agent. Your job is to implement the assigned unit as specified and stay within its scope. \
                You may read files, write files, and run shell commands. Inspect the relevant files before making claims or changes, \
                use fast scoped checks for local feedback while implementing, and record meaningful progress or failure context with native workflow updates. \
                Do not declare success if commands or checks fail; report the exact blocker and the next useful action. \
                Treat workflow tasks as execution contracts: use their scope, dependencies, acceptance criteria, and verify gate before broadening the work. \
                You may not create, run, or close unrelated work items — final verification and closure belong to the orchestrator workflow.",
            ),
            AgentMode::Orchestrator => Some(
                "You are an orchestrator agent. Use native workflows as your primary execution substrate for non-trivial work. \
                Inspect workflow state before making claims about work status, avoid duplicating or fragmenting existing tasks, and enrich existing workflows when that is cleaner than creating new ones. \
                Write detailed workflow steps, split larger efforts into child workflows with dependencies, dispatch workers through workflow run actions, and own the final verification, retry, and closure workflow. \
                Use the full workflow vocabulary when it helps: acceptance criteria, labels, dependencies, paths, decisions, checks, evidence, and artifacts. \
                Encode unresolved questions as decisions instead of burying ambiguity in prose. \
                When the conversation itself is producing durable plans, architecture, migrations, or implementation structure, externalize that structure into workflows during the conversation rather than waiting until the end. \
                Prefer native workflow actions and schema-checked updates over shell or direct file edits for maintaining the work graph. \
                You may not read or write files directly — create and dispatch workflow tasks for all file work. \
                Update workflows with concrete failure context and do not retry unchanged failed plans. \
                You are responsible for task structure, completeness, and verify quality.",
            ),
            AgentMode::Planner => Some(
                "You are a planner agent. Your job is to decompose work into workflow tasks. \
                Read enough code and context to ground the plan, cite concrete files or constraints when they matter, \
                and make dependencies, sequencing, acceptance criteria, and verify commands explicit. \
                Write worker-ready task descriptions that include current state, concrete steps, file paths with intent, embedded context, scope boundaries, and what not to do. \
                Record unresolved questions as decisions when autonomous execution would otherwise require guessing. \
                Externalize durable planning structure into workflows during the conversation, not only after the plan is complete. \
                Prefer schema-checked workflow updates to keep the graph current as ideas sharpen. \
                You may read files and update workflow plans, but you may not run them — \
                a human or orchestrator will approve execution.",
            ),
            AgentMode::Reviewer => Some(
                "You are a reviewer agent. Your job is to read code and report findings. \
                Ground findings in inspected code, cite exact files or symbols when useful, and distinguish confirmed issues from possible concerns. \
                You may not write files, run commands, or use workflow tooling.",
            ),
            AgentMode::Auditor => Some(
                "You are an auditor agent. Your job is to inspect code and workflow state \
                and produce structured reports. Ground conclusions in inspected evidence, cite the relevant files or workflow objects, \
                and clearly separate facts, risks, and open questions. You may read files and workflow status, \
                but you may not modify anything.",
            ),
        }
    }
}

/// Write tool overwrite safety policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteOverwritePolicy {
    /// Allow overwrites but return warnings for unread/stale files.
    #[default]
    Warn,
    /// Block overwrites unless the file was read in this session and is not stale.
    RequireRead,
    /// Block only stale overwrites; unread overwrites still warn.
    BlockStale,
    /// Block all overwrites. New file creation is still allowed.
    Deny,
}

/// File write tool settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WriteConfig {
    #[serde(default)]
    pub overwrite_policy: WriteOverwritePolicy,
}

/// Shell backend selection for the Bash tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ShellBackend {
    /// Use a POSIX-compatible shell command. Defaults to `bash -c`.
    #[default]
    Sh,
    /// Use the rush library API (`rush::run`). Falls back to the configured shell if
    /// the `rush-backend` feature is not compiled in.
    Rush,
    /// Connect to a running rush daemon over Unix socket. Falls back to the configured shell
    /// if the daemon is not reachable.
    RushDaemon,
}

/// Shell-related configuration for the Bash tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShellConfig {
    /// Which shell backend to use. Defaults to the standard command shell.
    #[serde(default)]
    pub backend: ShellBackend,
    /// Shell executable used for command execution. Defaults to `bash`.
    #[serde(default)]
    pub command: Option<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            backend: ShellBackend::Sh,
            command: None,
        }
    }
}

/// Concrete capability policy for the shipped Lua extension runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaCapabilityPolicy {
    pub allow_native_tool_calls: bool,
    pub allow_shell_exec: bool,
    pub allow_http: bool,
    pub allow_secrets: bool,
    pub allowed_env: HashSet<String>,
}

impl Default for LuaCapabilityPolicy {
    fn default() -> Self {
        Self {
            allow_native_tool_calls: true,
            allow_shell_exec: false,
            allow_http: false,
            allow_secrets: false,
            allowed_env: HashSet::new(),
        }
    }
}

/// Configuration for the shipped Lua extension runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LuaConfig {
    /// Whether `imp.tool()` calls from Lua are allowed.
    pub allow_native_tool_calls: Option<bool>,
    /// Whether `imp.exec()` is allowed.
    pub allow_shell_exec: Option<bool>,
    /// Whether `imp.http.*` is allowed.
    pub allow_http: Option<bool>,
    /// Whether `imp.secret()` / `imp.secret_fields()` are allowed.
    pub allow_secrets: Option<bool>,
    /// Env vars Lua extensions may read through `imp.env()`.
    pub allowed_env: Option<Vec<String>>,
}

impl LuaConfig {
    #[must_use]
    pub fn resolve_policy(&self, mode: AgentMode) -> LuaCapabilityPolicy {
        let mut policy = LuaCapabilityPolicy::default();
        // Worker agents inherit the user's explicit Lua secret capability so
        // agent-invoked extension tools behave the same as the parent session.
        if matches!(mode, AgentMode::Worker) {
            policy.allow_secrets = self.allow_secrets.unwrap_or(false);
        }
        if let Some(value) = self.allow_native_tool_calls {
            policy.allow_native_tool_calls = value;
        }
        if let Some(value) = self.allow_shell_exec {
            policy.allow_shell_exec = value;
        }
        if let Some(value) = self.allow_http {
            policy.allow_http = value;
        }
        if let Some(value) = self.allow_secrets {
            policy.allow_secrets = value;
        }
        if let Some(values) = &self.allowed_env {
            policy.allowed_env = values.iter().cloned().collect();
        }
        policy
    }
}

/// Native command secret-injection policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretsConfig {
    #[serde(default)]
    pub commands: CommandSecretsConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandSecretsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed: Vec<AllowedCommandSecret>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowedCommandSecret {
    pub name: String,
}

/// Policy decision configured for a capability class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyAction {
    #[default]
    Allow,
    Ask,
    Deny,
}

/// Explicit runtime policy settings for tool and capability decisions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyConfig {
    /// Whether side-effecting tool actions are allowed at all.
    #[serde(default = "default_true")]
    pub allow_side_effects: bool,
    /// Policy for writes inside the current workspace.
    #[serde(default)]
    pub workspace_writes: PolicyAction,
    /// Policy for writes outside the current workspace. Defaults to deny.
    #[serde(default = "default_policy_deny")]
    pub outside_workspace_writes: PolicyAction,
    /// Policy for shell/execute actions.
    #[serde(default)]
    pub shell: PolicyAction,
    /// Policy for network actions. Defaults to allow for normal user ergonomics.
    #[serde(default)]
    pub network: PolicyAction,
    /// Policy for browser input such as click, fill, and key actions.
    /// Defaults to deny; set to allow only for trusted interactive sessions.
    #[serde(default = "default_policy_deny")]
    pub browser_input: PolicyAction,
    /// Policy for secret reveal/direct secret access. Defaults to deny.
    #[serde(default = "default_policy_deny")]
    pub secrets: PolicyAction,
    /// Policy for extension-declared network capability. Defaults to deny.
    #[serde(default = "default_policy_deny")]
    pub extension_network: PolicyAction,
    /// Deny actions that would require approval in noninteractive contexts.
    #[serde(default)]
    pub deny_approval_required: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_side_effects: true,
            workspace_writes: PolicyAction::Allow,
            outside_workspace_writes: PolicyAction::Deny,
            shell: PolicyAction::Allow,
            network: PolicyAction::Allow,
            browser_input: PolicyAction::Deny,
            secrets: PolicyAction::Deny,
            extension_network: PolicyAction::Deny,
            deny_approval_required: false,
        }
    }
}

fn default_policy_deny() -> PolicyAction {
    PolicyAction::Deny
}

impl std::fmt::Display for PolicyAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            PolicyAction::Allow => "allow",
            PolicyAction::Ask => "ask",
            PolicyAction::Deny => "deny",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowScopePreference {
    #[default]
    Project,
    Root,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowRunConfig {
    /// Whether native workflow runs should return immediately by default.
    #[serde(default = "default_enabled")]
    pub background: bool,
    /// Number of units to run in parallel by default.
    #[serde(default = "default_workflow_run_jobs")]
    pub max_workers: u32,
    /// Continue running other ready units after one unit fails.
    #[serde(default)]
    pub continue_after_failure: bool,
    /// Review or evaluate units after a native run completes.
    #[serde(default)]
    pub review_after_run: bool,
}

impl Default for WorkflowRunConfig {
    fn default() -> Self {
        Self {
            background: true,
            max_workers: 4,
            continue_after_failure: false,
            review_after_run: false,
        }
    }
}

impl WorkflowRunConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_enabled() -> bool {
    true
}

fn default_workflow_run_jobs() -> u32 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowConfig {
    /// Default workflow selection for native workflow calls.
    #[serde(default)]
    pub scope: WorkflowScopePreference,
    /// Whether successful workflow close operations should auto-commit workflow changes.
    #[serde(default)]
    pub auto_commit: bool,
    /// Whether workflows should close completed parent steps after closing a child.
    #[serde(default = "default_true")]
    pub auto_close_parent: bool,
    /// Default verify timeout, in seconds, for close/create/verify flows.
    #[serde(default)]
    pub verify_timeout: Option<u64>,
    /// Native workflow run defaults.
    #[serde(default, skip_serializing_if = "WorkflowRunConfig::is_default")]
    pub run: WorkflowRunConfig,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            scope: WorkflowScopePreference::Project,
            auto_commit: false,
            auto_close_parent: true,
            verify_timeout: None,
            run: WorkflowRunConfig::default(),
        }
    }
}

/// Top-level configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Default model (alias or full ID).
    pub model: Option<String>,

    /// Default thinking level.
    pub thinking: Option<ThinkingLevel>,

    /// Default max output tokens per response.
    pub max_tokens: Option<u32>,

    /// Maximum agent turns.
    pub max_turns: Option<u32>,

    /// Active tool names (None = all).
    pub tools: Option<Vec<String>>,

    /// Named roles.
    #[serde(default)]
    pub roles: HashMap<String, RoleDef>,

    /// Hook definitions.
    #[serde(default)]
    pub hooks: Vec<HookDef>,

    /// Context management settings.
    #[serde(default)]
    pub context: ContextConfig,

    /// Shell backend settings.
    #[serde(default)]
    pub shell: ShellConfig,

    /// Write tool settings.
    #[serde(default)]
    pub write: WriteConfig,

    /// Engineering guardrails — profile-aware guidance and post-write checks.
    #[serde(default)]
    pub guardrails: GuardrailConfig,

    /// Agent mode — controls tool and workflow action access.
    #[serde(default)]
    pub mode: AgentMode,

    /// Enabled models for the model selector (None = show all).
    /// Entries can be canonical IDs or aliases (e.g. "sonnet", "claude-sonnet-4-6").
    #[serde(default)]
    pub enabled_models: Option<Vec<String>>,

    /// Theme name ("default", "light", or custom).
    pub theme: Option<String>,

    /// Learning loop settings (memory, skill nudges).
    #[serde(default)]
    pub learning: LearningConfig,

    /// UI display settings.
    #[serde(default)]
    pub ui: UiConfig,

    /// Browser automation settings.
    #[serde(default)]
    pub browser: BrowserConfig,

    /// Web tool settings.
    #[serde(default)]
    pub web: WebConfig,

    /// Workflow tool settings.
    #[serde(default)]
    pub workflow: WorkflowConfig,

    /// Shipped Lua extension runtime policy.
    #[serde(default)]
    pub lua: LuaConfig,

    /// Secret injection policy for native command execution.
    #[serde(default)]
    pub secrets: SecretsConfig,

    /// Explicit runtime policy settings for tool and capability decisions.
    #[serde(default)]
    pub policy: PolicyConfig,
}

// ── UI configuration ────────────────────────────────────────────

/// How the sidebar displays tool calls.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarStyle {
    /// Contextual inspector for the selected tool call.
    #[default]
    Inspector,
    /// Chronological stream of tool calls with inline results.
    Stream,
    /// Master-detail split: tool list (top) + selected output (bottom).
    Split,
}

/// How much tool output to show per tool call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolOutputDisplay {
    /// Show all output lines (scrollable).
    Full,
    /// Show first N lines per tool (configurable via `tool_output_lines`).
    #[default]
    Compact,
    /// Headers only — expand on click/enter.
    Collapsed,
}

/// How tool calls appear inside the chat transcript.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatToolDisplay {
    /// Show tool calls inline where they occurred, preserving chronological order.
    Interleaved,
    /// Show a compact header in chat and leave details to the sidebar.
    #[default]
    Summary,
    /// Hide tool calls in chat entirely.
    Hidden,
}

/// UI animation intensity.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimationLevel {
    /// No animated motion; show static state labels only.
    None,
    /// Basic spinner-only motion.
    Spinner,
    /// Restrained motion with concise state-specific labels.
    #[default]
    #[serde(alias = "full")]
    Minimal,
}

/// Auto-continue policy for imp-local follow-on work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ContinuePolicy {
    /// Never auto-continue on imp's own.
    #[default]
    Disabled,
    /// Only auto-continue when the runtime evidence is especially strong.
    Conservative,
    /// Auto-continue on clear, visible, workflow-backed next steps.
    Balanced,
    /// More willing to auto-continue when the local heuristic says confidence is high.
    Aggressive,
}

/// UI display configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiConfig {
    /// Sidebar layout style.
    #[serde(default)]
    pub sidebar_style: SidebarStyle,

    /// How much tool output to show.
    #[serde(default)]
    pub tool_output: ToolOutputDisplay,

    /// Max lines per tool in compact mode. Default: 10.
    #[serde(default = "default_tool_output_lines")]
    pub tool_output_lines: usize,

    /// Max lines the read tool returns before truncating. 0 disables line
    /// truncation for file reads. Default: 500.
    #[serde(default = "default_read_max_lines")]
    pub read_max_lines: usize,

    /// Sidebar width as percentage of screen (20-80). Default: 40.
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u16,

    /// Word-wrap long lines in tool output. Default: true.
    #[serde(default = "default_true")]
    pub word_wrap: bool,

    /// Animation intensity for the TUI. Default: minimal.
    #[serde(default)]
    pub animations: AnimationLevel,

    /// Legacy compatibility flag for older configs. Prefer `chat_tool_display`.
    #[serde(default)]
    pub hide_tools_in_chat: bool,

    /// How tool calls should appear in the chat transcript.
    #[serde(default)]
    pub chat_tool_display: ChatToolDisplay,

    /// Auto-open the sidebar on the first tool call. Default: true.
    #[serde(default = "default_true")]
    pub auto_open_sidebar: bool,

    /// Minimum terminal width to auto-open sidebar. Default: 120.
    #[serde(default = "default_sidebar_auto_open_width")]
    pub sidebar_auto_open_width: u16,

    /// Number of thinking lines to show in the rolling tail. Default: 5.
    #[serde(default = "default_thinking_lines")]
    pub thinking_lines: usize,

    /// Number of streaming tool output lines to retain. Default: 5.
    #[serde(default = "default_streaming_lines")]
    pub streaming_lines: usize,

    /// Mouse wheel scroll speed in lines. Default: 3.
    #[serde(default = "default_mouse_scroll_lines")]
    pub mouse_scroll_lines: usize,

    /// Keyboard/page scroll speed in lines. Default: 20.
    #[serde(default = "default_keyboard_scroll_lines")]
    pub keyboard_scroll_lines: usize,

    /// Deprecated: mouse capture is now always enabled. This field is retained
    /// only for backwards-compatible deserialization of existing config files.
    #[serde(default)]
    #[doc(hidden)]
    pub mouse_capture: bool,

    /// Show timestamps in chat. Default: false.
    #[serde(default)]
    pub show_timestamps: bool,

    /// Show cost in the top bar. Default: true.
    #[serde(default = "default_true")]
    pub show_cost: bool,

    /// Show context usage in the top bar. Default: true.
    #[serde(default = "default_true")]
    pub show_context_usage: bool,

    /// Emit a terminal bell when an agent run fully completes in the TUI.
    /// Default: true.
    #[serde(default = "default_true")]
    pub notify_on_agent_complete: bool,

    /// Policy for imp-local automatic continuation after a visible, high-confidence turn.
    /// Default: disabled.
    #[serde(default)]
    pub continue_policy: ContinuePolicy,

    /// Maximum number of automatic Build-mode task turns before pausing.
    /// Default: 20.
    #[serde(default = "default_build_auto_turn_budget")]
    pub build_auto_turn_budget: u32,

    /// Maximum number of automatic Improve-mode research turns before pausing.
    /// Default: 5.
    #[serde(default = "default_improve_auto_turn_budget")]
    pub improve_auto_turn_budget: u32,

    /// Maximum number of `/loop` automatic turns before pausing.
    /// 0 disables the imp-level loop cap. Default: 0.
    #[serde(default)]
    pub loop_turn_budget: u32,
}

fn default_tool_output_lines() -> usize {
    10
}
fn default_read_max_lines() -> usize {
    500
}
fn default_sidebar_width() -> u16 {
    40
}
fn default_sidebar_auto_open_width() -> u16 {
    120
}
fn default_thinking_lines() -> usize {
    5
}
fn default_streaming_lines() -> usize {
    5
}
fn default_mouse_scroll_lines() -> usize {
    3
}
fn default_keyboard_scroll_lines() -> usize {
    20
}
fn default_build_auto_turn_budget() -> u32 {
    20
}
fn default_improve_auto_turn_budget() -> u32 {
    5
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            sidebar_style: SidebarStyle::default(),
            tool_output: ToolOutputDisplay::default(),
            tool_output_lines: default_tool_output_lines(),
            read_max_lines: default_read_max_lines(),
            sidebar_width: default_sidebar_width(),
            word_wrap: default_true(),
            animations: AnimationLevel::default(),
            hide_tools_in_chat: false,
            chat_tool_display: ChatToolDisplay::default(),
            auto_open_sidebar: default_true(),
            sidebar_auto_open_width: default_sidebar_auto_open_width(),
            thinking_lines: default_thinking_lines(),
            streaming_lines: default_streaming_lines(),
            mouse_scroll_lines: default_mouse_scroll_lines(),
            keyboard_scroll_lines: default_keyboard_scroll_lines(),
            mouse_capture: false,
            show_timestamps: false,
            show_cost: true,
            show_context_usage: true,
            notify_on_agent_complete: true,
            continue_policy: ContinuePolicy::Disabled,
            build_auto_turn_budget: default_build_auto_turn_budget(),
            improve_auto_turn_budget: default_improve_auto_turn_budget(),
            loop_turn_budget: 0,
        }
    }
}

impl UiConfig {
    pub fn effective_chat_tool_display(&self) -> ChatToolDisplay {
        if self.hide_tools_in_chat && self.sidebar_style != SidebarStyle::Inspector {
            ChatToolDisplay::Hidden
        } else if self.sidebar_style == SidebarStyle::Inspector {
            ChatToolDisplay::Summary
        } else {
            self.chat_tool_display
        }
    }
}

/// Learning loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearningConfig {
    /// Master switch for memory + skill nudges. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Tool call count before suggesting skill creation. Default: 8.
    #[serde(default = "default_nudge_threshold")]
    pub skill_nudge_threshold: u32,

    /// Character limit for memory.md. Default: 2200.
    #[serde(default = "default_memory_limit")]
    pub memory_char_limit: usize,

    /// Character limit for user.md. Default: 1400.
    #[serde(default = "default_user_limit")]
    pub user_char_limit: usize,
}

fn default_true() -> bool {
    true
}
fn default_nudge_threshold() -> u32 {
    8
}
fn default_memory_limit() -> usize {
    2200
}
fn default_user_limit() -> usize {
    1400
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            skill_nudge_threshold: default_nudge_threshold(),
            memory_char_limit: default_memory_limit(),
            user_char_limit: default_user_limit(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AutoCompactionMode {
    /// Automatic context compaction is disabled; manual `/compact` only.
    #[default]
    Disabled,
    /// Reserved placeholder for future near-threshold auto-compaction.
    NearThreshold,
    /// Reserved placeholder for future aggressive auto-compaction.
    Aggressive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoCompactionConfig {
    /// Mode selection for automatic context compaction.
    #[serde(default)]
    pub mode: AutoCompactionMode,

    /// Trigger ratio for near-threshold auto-compaction. Defaults to 0.90.
    #[serde(default = "default_auto_compaction_trigger_ratio")]
    pub trigger_ratio: f64,

    /// Target ratio after auto-compaction. Defaults to 0.70.
    #[serde(default = "default_auto_compaction_target_ratio")]
    pub target_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummarizerConfig {
    /// Summarizer model. `default` means use the active conversation model.
    #[serde(default = "default_summarizer_model")]
    pub model: String,

    /// Reserved token budget for summarization/recovery prompts.
    #[serde(default = "default_summarizer_reserve_tokens")]
    pub reserve_tokens: u32,

    /// Target size for generated summaries.
    #[serde(default = "default_summarizer_target_summary_tokens")]
    pub target_summary_tokens: u32,

    /// Fully replaces imp's built-in summarization prompt when set.
    #[serde(default)]
    pub prompt: Option<String>,
}

fn default_auto_compaction_trigger_ratio() -> f64 {
    0.90
}

fn default_auto_compaction_target_ratio() -> f64 {
    0.70
}

fn default_summarizer_model() -> String {
    "default".to_string()
}

fn default_summarizer_reserve_tokens() -> u32 {
    50_000
}

fn default_summarizer_target_summary_tokens() -> u32 {
    40_000
}

impl Default for AutoCompactionConfig {
    fn default() -> Self {
        Self {
            mode: AutoCompactionMode::NearThreshold,
            trigger_ratio: default_auto_compaction_trigger_ratio(),
            target_ratio: default_auto_compaction_target_ratio(),
        }
    }
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self {
            model: default_summarizer_model(),
            reserve_tokens: default_summarizer_reserve_tokens(),
            target_summary_tokens: default_summarizer_target_summary_tokens(),
            prompt: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextConfig {
    /// Mask old tool outputs at this ratio (default: 0.6).
    pub observation_mask_threshold: f64,

    /// Keep last N turns unmasked (default: 10).
    pub mask_window: usize,

    /// Auto-compaction settings. Enabled by default.
    #[serde(default)]
    pub auto_compaction: AutoCompactionConfig,

    /// Summarizer settings for semantic context compaction.
    #[serde(default)]
    pub summarizer: SummarizerConfig,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            observation_mask_threshold: 0.6,
            mask_window: 10,
            auto_compaction: AutoCompactionConfig::default(),
            summarizer: SummarizerConfig::default(),
        }
    }
}

impl Config {
    /// Load config from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Resolve the full config by merging: defaults < user < project < env < CLI.
    pub fn resolve(user_config_dir: &Path, project_dir: Option<&Path>) -> Result<Self> {
        let mut config = Self::default();

        // User config
        let user_path = user_config_dir.join("config.toml");
        if user_path.exists() {
            let user = Self::load(&user_path)?;
            config.merge(user);
        }

        // Project config
        if let Some(project) = project_dir {
            let project_path = project.join(".imp").join("config.toml");
            if project_path.exists() {
                let project = Self::load(&project_path)?;
                config.merge(project);
            }
        }

        // Env overrides
        if let Ok(model) = std::env::var("IMP_MODEL") {
            config.model = Some(model);
        }
        if let Ok(thinking) = std::env::var("IMP_THINKING") {
            config.thinking = parse_thinking_level(&thinking);
        }
        if let Ok(max_tokens) = std::env::var("IMP_MAX_TOKENS") {
            if let Ok(parsed) = max_tokens.parse::<u32>() {
                config.max_tokens = Some(parsed);
            }
        }
        if let Ok(mode) = std::env::var("IMP_MODE") {
            if let Some(m) = parse_agent_mode(&mode) {
                config.mode = m;
            }
        }
        if let Ok(provider) = std::env::var("IMP_WEB_PROVIDER") {
            config.web.search_provider = match provider.to_lowercase().as_str() {
                "tavily" => Some(crate::tools::web::types::SearchProvider::Tavily),
                "exa" => Some(crate::tools::web::types::SearchProvider::Exa),
                "linkup" => Some(crate::tools::web::types::SearchProvider::Linkup),
                "perplexity" => Some(crate::tools::web::types::SearchProvider::Perplexity),
                _ => config.web.search_provider,
            };
        }

        Ok(config)
    }

    fn merge(&mut self, other: Config) {
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.thinking.is_some() {
            self.thinking = other.thinking;
        }
        if other.max_tokens.is_some() {
            self.max_tokens = other.max_tokens;
        }
        if other.max_turns.is_some() {
            self.max_turns = other.max_turns;
        }
        if other.tools.is_some() {
            self.tools = other.tools;
        }
        if other.context != ContextConfig::default() {
            self.context = other.context;
        }
        if other.shell != ShellConfig::default() {
            self.shell = other.shell;
        }
        self.guardrails.merge(other.guardrails);
        if other.mode != AgentMode::default() {
            self.mode = other.mode;
        }
        if other.enabled_models.is_some() {
            self.enabled_models = other.enabled_models;
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
        if other.learning != LearningConfig::default() {
            self.learning = other.learning;
        }
        if other.ui != UiConfig::default() {
            self.ui = other.ui;
        }
        if other.browser != BrowserConfig::default() {
            self.browser = other.browser;
        }
        if other.web != WebConfig::default() {
            self.web = other.web;
        }
        if other.workflow != WorkflowConfig::default() {
            self.workflow = other.workflow;
        }
        if other.lua != LuaConfig::default() {
            self.lua = other.lua;
        }
        if other.secrets != SecretsConfig::default() {
            self.secrets = other.secrets;
        }
        self.roles.extend(other.roles);
        self.hooks.extend(other.hooks);
    }

    /// Default user config directory.
    pub fn user_config_dir() -> PathBuf {
        storage::global_root()
    }

    /// Default session directory.
    pub fn session_dir() -> PathBuf {
        storage::global_sessions_dir()
    }

    /// Resolve built-in roles plus config overrides and validate them.
    pub fn role_registry(&self) -> std::result::Result<RoleRegistry, RoleRegistryError> {
        RoleRegistry::from_overrides(self.roles.clone())
    }

    /// Save config to a TOML file. Creates parent directories if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content =
            toml::to_string_pretty(self).map_err(|e| crate::error::Error::Config(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Path to the user config.toml file.
    pub fn user_config_path() -> PathBuf {
        storage::global_config_path()
    }
}

fn parse_agent_mode(s: &str) -> Option<AgentMode> {
    AgentMode::from_name(s)
}

fn parse_thinking_level(s: &str) -> Option<ThinkingLevel> {
    match s.to_lowercase().as_str() {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::XHigh),
        _ => None,
    }
}

#[cfg(test)]
#[path = "config/tests.rs"]
mod tests;
