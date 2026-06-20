use serde::{Deserialize, Serialize};

use super::default_true;

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
