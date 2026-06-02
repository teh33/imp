use super::*;

impl App {
    pub(super) fn prepare_for_interactive(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = imp_core::storage::reconcile_legacy_into_global_root();
        // Load Lua extensions (for slash commands and tool registration)
        self.reload_lua_extensions();

        // Check for first-run welcome flow
        let config_dir = Config::user_config_dir();
        let auth_path = imp_core::storage::global_auth_path();
        if needs_welcome(&config_dir, &auth_path) {
            let all_models = self.model_registry.list().to_vec();
            self.mode = UiMode::Welcome(WelcomeState::new(&all_models));
        }

        Ok(())
    }

    pub(super) fn maybe_notify_agent_completion(&mut self) {
        if self.is_streaming {
            return;
        }
        if self.completed_turns_in_run == 0 {
            return;
        }
        if self.suppress_completion_notification {
            self.completed_turns_in_run = 0;
            self.suppress_completion_notification = false;
            return;
        }
        if !self.config.ui.notify_on_agent_complete {
            self.completed_turns_in_run = 0;
            return;
        }

        let _ = ring_terminal_bell();
        self.completed_turns_in_run = 0;
    }

    pub(super) fn stop_active_work(&mut self) {
        if self.is_streaming || self.agent_task.is_some() {
            if let Some(ref handle) = self.agent_handle {
                let _ = handle.command_tx.try_send(AgentCommand::Cancel);
                handle
                    .cancel_token
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(task) = self.agent_task.take() {
                task.abort();
            }
            if let Some(task) = self.agent_event_task.take() {
                task.abort();
            }
            self.agent_handle = None;
            self.is_streaming = false;
            self.streaming_anchor_user_index = None;
            if let Some(last) = self.latest_streaming_message_mut() {
                last.is_streaming = false;
            }
        }

        self.clear_pending_agent_turn();
        self.message_queue.clear();
        self.loop_state = None;
        self.suppress_completion_notification = true;
        if let Some(run_id) = self
            .active_workflow_run
            .as_ref()
            .map(|run| run.run_id.clone())
        {
            match stop_workflow_run(&run_id) {
                Ok(Some(summary)) => {
                    self.active_workflow_run = Some(summary);
                    self.push_system_msg(&format!(
                        "Stopped active workflow run {run_id}. External workers may need manual cleanup."
                    ));
                }
                Ok(None) => {
                    self.push_system_msg(&format!("Active workflow run {run_id} was not found."))
                }
                Err(err) => {
                    self.push_system_msg(&format!("Could not stop workflow run {run_id}: {err}"))
                }
            }
        }

        self.push_system_msg("Stopped active imp work.");
    }

    pub(super) fn cancel_pending_work(&mut self) -> bool {
        let had_pending = self.pending_agent_prompt.is_some();
        let had_queued = !self.message_queue.is_empty();
        let had_loop = self.loop_state.is_some();
        if !(had_pending || had_queued || had_loop) {
            return false;
        }

        self.clear_pending_agent_turn();
        self.message_queue.clear();
        self.loop_state = None;
        let mut parts = Vec::new();
        if had_pending {
            parts.push("pending prompt");
        }
        if had_queued {
            parts.push("queued follow-up");
        }
        if had_loop {
            parts.push("loop");
        }
        self.push_system_msg(&format!("Cleared {}.", parts.join(", ")));
        true
    }

    pub(super) fn handle_cancel(&mut self) {
        if !self.editor.is_empty() {
            // First Ctrl+C: clear editor
            self.editor.clear();
            self.ctrl_c_count = 0;
        } else if self.is_streaming || self.agent_task.is_some() {
            let already_cancelled = self.agent_handle.as_ref().is_some_and(|handle| {
                handle
                    .cancel_token
                    .load(std::sync::atomic::Ordering::Relaxed)
            });
            if already_cancelled {
                if let Some(task) = self.agent_task.take() {
                    task.abort();
                }
                if let Some(task) = self.agent_event_task.take() {
                    task.abort();
                }
                self.agent_handle = None;
            } else if let Some(ref handle) = self.agent_handle {
                let _ = handle.command_tx.try_send(AgentCommand::Cancel);
                handle
                    .cancel_token
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.suppress_completion_notification = true;
            self.is_streaming = false;
            self.streaming_anchor_user_index = None;
            if let Some(last) = self.latest_streaming_message_mut() {
                last.is_streaming = false;
            }
            self.ctrl_c_count = 0;
        } else if self.cancel_pending_work() {
            self.ctrl_c_count = 0;
        } else {
            // Third: quit
            self.ctrl_c_count += 1;
            if self.ctrl_c_count >= 2 {
                self.running = false;
            }
        }
    }
}
