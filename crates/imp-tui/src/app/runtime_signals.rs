use imp_core::Error as ImpCoreError;

use super::{
    format_error_for_display, lua_result_requests_restart, runtime_signal_kind,
    strip_lua_restart_directive, App, LoginTaskExit, RepoStatsState, RuntimeSignal, UiMode,
    MAX_RUNTIME_SIGNALS_PER_TICK, MAX_UI_REQUESTS_PER_TICK,
};

impl App {
    pub(super) async fn pump_runtime_signals(&mut self) {
        let signals = self.collect_runtime_signals().await;
        for signal in signals {
            self.handle_runtime_signal(signal);
        }
    }

    pub(super) async fn collect_runtime_signals(&mut self) -> Vec<RuntimeSignal> {
        let mut signals = Vec::new();

        if let Some(handle) = self.agent_handle.as_mut() {
            while signals.len() < MAX_RUNTIME_SIGNALS_PER_TICK {
                match handle.event_rx.try_recv() {
                    Ok(event) => signals.push(RuntimeSignal::AgentEvent(event)),
                    Err(_) => break,
                }
            }
        }

        while signals.len() < MAX_RUNTIME_SIGNALS_PER_TICK {
            match self.runtime_signal_rx.try_recv() {
                Ok(signal) => signals.push(signal),
                Err(_) => break,
            }
        }

        let agent_task_finished = self
            .agent_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if agent_task_finished {
            if let Some(task) = self.agent_task.take() {
                let outcome = match task.await {
                    Ok(Ok(())) | Ok(Err(ImpCoreError::Cancelled)) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(error) => Err(format!("Internal agent task failure: {error}")),
                };

                // Drain all bridged events after confirmed completion. The agent can finish with
                // final events already queued in event_rx; if we clear/abort the bridge first,
                // late ToolExecutionEnd / TurnEnd / AgentEnd events are lost. The agent task has
                // completed, so the sender side is dropped and the bridge should terminate after
                // forwarding the remaining events.
                if let Some(task) = self.agent_event_task.take() {
                    let _ = task.await;
                }
                while let Ok(signal) = self.runtime_signal_rx.try_recv() {
                    signals.push(signal);
                }

                match outcome {
                    Ok(()) => signals.push(RuntimeSignal::AgentTaskCompleted),
                    Err(error) => signals.push(RuntimeSignal::AgentTaskFailed(error)),
                }
            }
        }

        let compaction_task_finished = self
            .compaction_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if compaction_task_finished {
            if let Some(task) = self.compaction_task.take() {
                match task.await {
                    Ok(Ok(summary)) => {
                        signals.push(RuntimeSignal::CompactionTaskCompleted(summary))
                    }
                    Ok(Err(error)) => signals.push(RuntimeSignal::CompactionTaskFailed(error)),
                    Err(error) => signals.push(RuntimeSignal::CompactionTaskFailed(format!(
                        "Internal compaction task failure: {error}"
                    ))),
                }
            }
        }

        let checkpoint_task_finished = self
            .checkpoint_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if checkpoint_task_finished {
            if let Some(task) = self.checkpoint_task.take() {
                match task.await {
                    Ok(Ok(())) => signals.push(RuntimeSignal::CheckpointTaskCompleted),
                    Ok(Err(error)) => signals.push(RuntimeSignal::CheckpointTaskFailed(error)),
                    Err(error) => signals.push(RuntimeSignal::CheckpointTaskFailed(format!(
                        "Internal checkpoint task failure: {error}"
                    ))),
                }
            }
        }

        let lua_command_task_finished = self
            .lua_command_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if lua_command_task_finished {
            if let Some(task) = self.lua_command_task.take() {
                match task.await {
                    Ok((command, Ok(result))) => {
                        if lua_result_requests_restart(result.as_deref()) {
                            signals
                                .push(RuntimeSignal::LuaCommandRestartRequested { command, result })
                        } else {
                            signals.push(RuntimeSignal::LuaCommandCompleted { command, result })
                        }
                    }
                    Ok((command, Err(error))) => {
                        signals.push(RuntimeSignal::LuaCommandFailed { command, error })
                    }
                    Err(error) => signals.push(RuntimeSignal::LuaCommandFailed {
                        command: "lua".to_string(),
                        error: format!("Lua command task failure: {error}"),
                    }),
                }
            }
        }

        let login_task_finished = self
            .login_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if login_task_finished {
            if let Some(task) = self.login_task.take() {
                match task.await {
                    Ok(LoginTaskExit::Success(message)) => {
                        signals.push(RuntimeSignal::LoginTaskSucceeded(message));
                    }
                    Ok(LoginTaskExit::Failed(message)) => {
                        signals.push(RuntimeSignal::LoginTaskFailed(message));
                    }
                    Err(error) => signals.push(RuntimeSignal::LoginTaskFailed(format!(
                        "Login task failure: {error}"
                    ))),
                }
            }
        }

        if let Some(rx) = self.ui_rx.as_mut() {
            let remaining_budget = MAX_RUNTIME_SIGNALS_PER_TICK.saturating_sub(signals.len());
            let ui_budget = remaining_budget.min(MAX_UI_REQUESTS_PER_TICK);
            for _ in 0..ui_budget {
                match rx.try_recv() {
                    Ok(req) => signals.push(RuntimeSignal::UiRequest(req)),
                    Err(_) => break,
                }
            }
        }

        signals
    }

    pub(super) fn handle_runtime_signal(&mut self, signal: RuntimeSignal) {
        let trace_kind = runtime_signal_kind(&signal);
        self.trace_tui(format!("runtime_signal_handle kind={trace_kind}"));
        match signal {
            RuntimeSignal::BrowserInstallCompleted(result) => {
                match result {
                    Ok(()) => self.push_system_msg("Lightpanda installation completed."),
                    Err(error) => {
                        self.push_error_msg(&format!("Lightpanda installation failed: {error}"))
                    }
                }
                self.run_browser_diagnostics();
                self.needs_redraw = true;
            }
            RuntimeSignal::BrowserDiagnosticCompleted(report) => {
                if let UiMode::Settings(settings) = &mut self.mode {
                    settings.browser.apply_diagnostic(report);
                }
                self.needs_redraw = true;
            }
            RuntimeSignal::AgentEvent(event) => self.handle_agent_event(event),
            RuntimeSignal::AgentTaskCompleted => {
                self.maybe_notify_agent_completion();
                // AgentEnd handling can synchronously spawn a replacement run via a
                // queued follow-up. Only clear the handle if no active task has
                // taken over by the time we process completion.
                let has_active_replacement = self
                    .agent_task
                    .as_ref()
                    .is_some_and(|task| !task.is_finished());
                if !has_active_replacement {
                    if let Some(task) = self.agent_event_task.take() {
                        task.abort();
                    }
                    self.agent_handle = None;
                    self.is_streaming = false;
                    self.schedule_checkpoint_if_due();
                }
            }
            RuntimeSignal::AgentTaskFailed(error) => {
                let has_active_replacement = self
                    .agent_task
                    .as_ref()
                    .is_some_and(|task| !task.is_finished());
                if !has_active_replacement {
                    if let Some(task) = self.agent_event_task.take() {
                        task.abort();
                    }
                    self.agent_handle = None;
                    self.is_streaming = false;
                    self.schedule_checkpoint_if_due();
                }
                self.present_agent_failure(error);
            }
            RuntimeSignal::CompactionTaskCompleted(summary) => {
                self.finish_manual_compaction(summary)
            }
            RuntimeSignal::CompactionTaskFailed(error) => {
                self.finish_compaction_status_message("Compaction failed.");
                self.push_error_msg(&format!("Compaction failed: {error}"));
            }
            RuntimeSignal::CheckpointTaskCompleted => {
                self.status_items.remove("compaction-checkpoint");
            }
            RuntimeSignal::CheckpointTaskFailed(error) => {
                self.status_items.remove("compaction-checkpoint");
                self.push_warning_msg(&format!("Compaction checkpoint failed: {error}"));
            }
            RuntimeSignal::LuaCommandCompleted { command, result } => {
                self.finish_lua_command_status_message(&format!("/{command} finished."));
                if let Some(text) = result {
                    self.push_system_msg(&text);
                }
            }
            RuntimeSignal::LuaCommandRestartRequested { command, result } => {
                self.finish_lua_command_status_message(&format!("/{command} finished."));
                if let Some(text) = result {
                    self.push_system_msg(&strip_lua_restart_directive(&text));
                }
                self.restart_after_lua_command();
            }
            RuntimeSignal::LuaCommandFailed { command, error } => {
                self.finish_lua_command_status_message(&format!("/{command} failed."));
                self.push_error_msg(&format!("Lua command error: {error}"));
            }
            RuntimeSignal::LoginUrlReady {
                provider,
                url,
                browser_opened,
            } => {
                if let UiMode::Welcome(ref mut state) = self.mode {
                    state.set_oauth_url(&provider, url, browser_opened);
                } else if !browser_opened {
                    self.push_system_msg(&format!(
                        "Unable to open a browser here. Open this URL manually: {url}"
                    ));
                }
            }
            RuntimeSignal::LoginTaskSucceeded(message) => {
                if let UiMode::Welcome(ref mut state) = self.mode {
                    state.mark_selected_provider_oauth_complete();
                } else {
                    self.push_system_msg(&message);
                }
            }
            RuntimeSignal::LoginTaskFailed(message) => {
                if let UiMode::Welcome(ref mut state) = self.mode {
                    state.set_key_error(&message);
                } else {
                    self.push_error_msg(&message);
                }
            }
            RuntimeSignal::SessionListLoaded(result) => self.finish_session_list_load(result),
            RuntimeSignal::SessionListFailed(error) => self.fail_session_list_load(error),
            RuntimeSignal::SessionOpened(result) => self.finish_session_open(result),
            RuntimeSignal::SessionOpenFailed(error) => self.push_error_msg(&error),
            RuntimeSignal::UserMessagePersisted {
                entry_id,
                persisted_session,
            } => self.finish_user_message_persist(entry_id, persisted_session),
            RuntimeSignal::UserMessagePersistFailed(error) => self.push_error_msg(&error),
            RuntimeSignal::AgentStartCompleted(result) => self.finish_agent_start(result),
            RuntimeSignal::AgentStartFailed(error) => self.fail_agent_start(error),
            RuntimeSignal::AgentStartStatus { key, text } => {
                if let Some(text) = text {
                    self.status_items.insert(key, text);
                } else {
                    self.status_items.remove(&key);
                }
            }
            #[cfg(feature = "mana-ui")]
            RuntimeSignal::ManaNavigatorLoaded(state) => self.finish_mana_navigator_load(state),
            #[cfg(feature = "mana-ui")]
            RuntimeSignal::ManaNavigatorLoadFailed { mana_dir, message } => {
                self.fail_mana_navigator_load(mana_dir, message);
            }
            RuntimeSignal::RepoStatsLoaded(result) => {
                self.startup_surface_metadata.repo_stats = Some(match result {
                    Ok(Some(stats)) => RepoStatsState::Ready(stats),
                    Ok(None) => RepoStatsState::Empty,
                    Err(_) => RepoStatsState::Failed,
                });
                self.needs_redraw = true;
            }
            RuntimeSignal::RepoStatsSkipped(state) => {
                self.startup_surface_metadata.repo_stats = Some(state);
                self.needs_redraw = true;
            }
            RuntimeSignal::UiRequest(req) => self.handle_ui_request(req),
        }
        self.needs_redraw = true;
    }

    fn present_agent_failure(&mut self, error: String) {
        self.completed_turns_in_run = 0;
        self.is_streaming = false;
        self.streaming_anchor_user_index = None;

        let display_error = format_error_for_display(&error);
        if self.last_agent_error.as_deref() != Some(display_error.as_str()) {
            if !self.replace_empty_streaming_with_error(&display_error) {
                self.push_error_msg(&display_error);
            }
            self.last_agent_error = Some(display_error);
        } else {
            self.replace_empty_streaming_with_error(&display_error);
        }
    }
}
