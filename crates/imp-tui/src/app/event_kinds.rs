use imp_core::agent::AgentEvent;

use super::RuntimeSignal;

pub(super) fn runtime_signal_kind(signal: &RuntimeSignal) -> &'static str {
    match signal {
        RuntimeSignal::BrowserInstallCompleted(_) => "browser_install_completed",
        RuntimeSignal::BrowserDiagnosticCompleted(_) => "browser_diagnostic_completed",
        RuntimeSignal::AgentEvent(_) => "agent_event",
        RuntimeSignal::AgentTaskCompleted => "agent_task_completed",
        RuntimeSignal::AgentTaskFailed(_) => "agent_task_failed",
        RuntimeSignal::CompactionTaskCompleted(_) => "compaction_completed",
        RuntimeSignal::CheckpointTaskCompleted(_) => "checkpoint_completed",
        RuntimeSignal::CheckpointTaskFailed { .. } => "checkpoint_failed",
        RuntimeSignal::CompactionTaskFailed(_) => "compaction_failed",
        RuntimeSignal::LuaCommandCompleted { .. } => "lua_command_completed",
        RuntimeSignal::LuaCommandRestartRequested { .. } => "lua_command_restart_requested",
        RuntimeSignal::LuaCommandFailed { .. } => "lua_command_failed",
        RuntimeSignal::LoginUrlReady { .. } => "login_url_ready",
        RuntimeSignal::LoginTaskSucceeded(_) => "login_task_succeeded",
        RuntimeSignal::LoginTaskFailed(_) => "login_task_failed",
        RuntimeSignal::SessionListLoaded(_) => "session_list_loaded",
        RuntimeSignal::SessionListFailed(_) => "session_list_failed",
        RuntimeSignal::SessionOpened(_) => "session_opened",
        RuntimeSignal::SessionOpenFailed(_) => "session_open_failed",
        RuntimeSignal::UserMessagePersisted { .. } => "user_message_persisted",
        RuntimeSignal::UserMessagePersistFailed(_) => "user_message_persist_failed",
        RuntimeSignal::AgentStartCompleted(_) => "agent_start_completed",
        RuntimeSignal::AgentStartFailed(_) => "agent_start_failed",
        RuntimeSignal::AgentStartStatus { .. } => "agent_start_status",
        #[cfg(feature = "mana-ui")]
        RuntimeSignal::ManaNavigatorLoaded(_) => "mana_navigator_loaded",
        #[cfg(feature = "mana-ui")]
        RuntimeSignal::ManaNavigatorLoadFailed { .. } => "mana_navigator_load_failed",
        RuntimeSignal::RepoStatsLoaded(_) => "repo_stats_loaded",
        RuntimeSignal::RepoStatsSkipped(_) => "repo_stats_skipped",
        RuntimeSignal::UiRequest(_) => "ui_request",
    }
}

pub(super) fn agent_event_kind(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::AgentStart { .. } => "agent_start",
        AgentEvent::TurnStart { .. } => "turn_start",
        AgentEvent::TurnAssessment { .. } => "turn_assessment",
        AgentEvent::MessageStart { .. } => "message_start",
        AgentEvent::MessageEnd { .. } => "message_end",
        AgentEvent::MessageDelta { .. } => "message_delta",
        AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
        AgentEvent::ToolOutputDelta { .. } => "tool_output_delta",
        AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        AgentEvent::AgentEnd { .. } => "agent_end",
        AgentEvent::Browser { .. } => "browser_event",
        AgentEvent::ContextUsageUpdated { .. } => "context_usage",
        AgentEvent::Warning { .. } => "warning",
        AgentEvent::RecoveryCheckpoint { .. } => "recovery_checkpoint",
        AgentEvent::WorktreeCreated { .. } => "worktree_created",
        AgentEvent::WorktreeDiffCaptured { .. } => "worktree_diff_captured",
        AgentEvent::WorktreeCloseout { .. } => "worktree_closeout",
        AgentEvent::EvidenceWritten { .. } => "evidence_written",
        AgentEvent::WorkflowControllerSnapshot { .. } => "workflow_controller_snapshot",
        AgentEvent::VerificationStarted { .. } => "verification_started",
        AgentEvent::VerificationCompleted { .. } => "verification_completed",
        AgentEvent::PolicyChecked { .. } => "policy_checked",
        AgentEvent::Timing { .. } => "timing",
        AgentEvent::TurnEnd { .. } => "turn_end",
        AgentEvent::Error { .. } => "error",
    }
}
