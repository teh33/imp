use super::*;

impl App {
    /// Reload Lua extensions: re-scan directories, re-create runtime, and update
    /// the stored runtime handle. Tools are not re-registered on the running
    /// agent (only new agents will pick them up), but commands become available
    /// immediately.
    pub(in crate::app) fn reload_lua_extensions(&mut self) {
        let user_config_dir = Config::user_config_dir();
        let policy = self
            .config
            .lua
            .resolve_policy(imp_core::config::AgentMode::Full);
        match imp_lua::reload(&user_config_dir, Some(&self.cwd), &policy) {
            Ok((rt, _exts)) => {
                self.lua_runtime = Some(Arc::new(Mutex::new(rt)));
            }
            Err(e) => {
                self.push_system_msg(&format!("Lua reload failed: {e}"));
                self.lua_runtime = None;
            }
        }
    }

    pub(in crate::app) fn lua_command_call_context(&self) -> imp_lua::LuaCallContext {
        let (update_tx, _update_rx) = tokio::sync::mpsc::channel(16);
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(16);
        let ui: Arc<dyn imp_core::ui::UserInterface> = self
            .lua_command_ui
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(imp_core::ui::NullInterface));
        imp_lua::LuaCallContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx,
            command_tx,
            ui,
            file_cache: Arc::new(imp_core::tools::FileCache::new()),
            checkpoint_state: Arc::new(imp_core::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(
                imp_core::tools::FileTracker::default(),
            )),
            anchor_store: Arc::new(imp_core::tools::AnchorStore::new()),
            lua_tool_loader: None,
            mode: imp_core::config::AgentMode::Full,
            read_max_lines: self.config.ui.read_max_lines,
            run_policy: Default::default(),
            config: Arc::new(self.config.clone()),
        }
    }

    /// Try to dispatch a slash command to a Lua extension handler.
    /// Returns `true` if a matching Lua command was found and executed.
    pub(in crate::app) fn try_lua_command(&mut self, cmd: &str) -> bool {
        let runtime = match &self.lua_runtime {
            Some(rt) => Arc::clone(rt),
            None => return false,
        };

        let guard = match runtime.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };

        // Find a command matching the typed name (first word)
        let cmd_name = cmd.split_whitespace().next().unwrap_or(cmd);
        let args = cmd.strip_prefix(cmd_name).unwrap_or("").trim();

        if !guard.has_command(cmd_name) {
            return false;
        }
        drop(guard);

        // Execute via LuaRuntime's helper (keeps mlua types internal) on a
        // background task so extension commands share /compact's non-blocking
        // transcript animation and completion flow.
        if self.lua_command_task.is_some() {
            self.push_system_msg("A Lua command is already running.");
            return true;
        }

        let command_label = cmd_name.to_string();
        let args = args.to_string();
        let call_ctx = self.lua_command_call_context();
        self.messages.push(DisplayMessage {
            role: MessageRole::Compaction,
            content: format!("Running /{command_label}…"),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: true,
            timestamp: imp_llm::now(),
        });
        self.auto_scroll = true;
        self.scroll_offset = 0;
        self.invalidate_chat_render_cache();

        let task_command = command_label.clone();
        let run_lua_command = move || {
            let result = match runtime.lock() {
                Ok(guard) => guard
                    .execute_command_with_context(&task_command, &args, Some(call_ctx))
                    .map_err(|error| error.to_string()),
                Err(_) => Err("Lua runtime lock poisoned".to_string()),
            };
            (task_command, result)
        };

        if tokio::runtime::Handle::try_current().is_ok() {
            self.lua_command_task = Some(tokio::task::spawn_blocking(run_lua_command));
        } else {
            let (command, result) = run_lua_command();
            let signal = match result {
                Ok(result) if lua_result_requests_restart(result.as_deref()) => {
                    RuntimeSignal::LuaCommandRestartRequested { command, result }
                }
                Ok(result) => RuntimeSignal::LuaCommandCompleted { command, result },
                Err(error) => RuntimeSignal::LuaCommandFailed { command, error },
            };
            self.handle_runtime_signal(signal);
        }
        true
    }
}
