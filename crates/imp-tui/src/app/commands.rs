use std::process::Command;

use super::*;

impl App {
    // ── Commands ────────────────────────────────────────────────

    pub(super) fn loop_label(&self) -> Option<String> {
        let state = self.loop_state.as_ref()?;
        Some(match state.budget {
            Some(budget) => format!("↻ loop {}/{}", state.completed_turns.min(budget), budget),
            None => format!("↻ loop {}", state.completed_turns),
        })
    }

    pub(super) fn loop_continue_message(&self) -> String {
        if let Some(visible) = self.pending_agent_visible_text.as_ref() {
            return visible.clone();
        }
        if let Some(scope) = self.active_workflow_scope.as_ref() {
            return format!(
                "Continue working on active mana scope {} until the requested outcome is complete, blocked, or no runnable work remains.",
                scope.id
            );
        }
        #[cfg(feature = "mana-ui")]
        if let Some(run) = self.active_workflow_run.as_ref() {
            return format!(
                "Continue supervising active workflow run {} until it is complete, blocked, or no runnable work remains.",
                run.run_id
            );
        }
        "continue".to_string()
    }

    pub(super) fn start_loop_command(&mut self, message: &str) {
        let message = message.trim();
        let message = if message.is_empty() || message.eq_ignore_ascii_case("continue") {
            self.loop_continue_message()
        } else {
            message.to_string()
        };
        let budget =
            (self.config.ui.loop_turn_budget > 0).then_some(self.config.ui.loop_turn_budget);
        self.loop_state = Some(LoopState {
            message,
            completed_turns: 0,
            budget,
        });
        match budget {
            Some(budget) => self.push_system_msg(&format!("Loop started: {budget} turn budget.")),
            None => self.push_system_msg("Loop started: no turn budget."),
        }
        self.queue_loop_continuation_if_ready();
    }

    pub(super) fn queue_loop_continuation_if_ready(&mut self) {
        if self.is_streaming
            || self.pending_agent_prompt.is_some()
            || !self.message_queue.is_empty()
        {
            return;
        }
        let Some(state) = self.loop_state.as_mut() else {
            return;
        };
        if let Some(budget) = state.budget {
            if state.completed_turns >= budget {
                self.loop_state = None;
                self.push_system_msg(&format!(
                    "Loop paused after {budget} turns. Use /loop <message> to start again."
                ));
                return;
            }
        }
        state.completed_turns += 1;
        let message = state.message.clone();
        let completed = state.completed_turns;
        let budget = state.budget;
        match budget {
            Some(budget) => self.push_system_msg(&format!("Loop: turn {completed}/{budget}")),
            None => self.push_system_msg(&format!("Loop: turn {completed}")),
        }
        self.enqueue_visible_agent_turn(message);
        self.needs_redraw = true;
    }

    pub(super) fn preloaded_lua_tools(&self) -> Option<ToolRegistry> {
        let policy = self.config.lua.resolve_policy(self.config.mode);
        let mut tools = ToolRegistry::new();
        let user_config_dir = imp_core::config::Config::user_config_dir();
        imp_lua::init_lua_extensions(&user_config_dir, Some(&self.cwd), &mut tools, &policy);
        Some(tools)
    }
    pub(super) fn agent_start_request(&mut self) -> AgentStartRequest {
        let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(16);
        let tui_ui = crate::tui_interface::TuiInterface::new(ui_tx.clone());
        self.lua_command_ui = Some(tui_ui);
        self.ui_rx = Some(ui_rx);

        AgentStartRequest {
            session: self.session.clone(),
            model_name: self.model_name.clone(),
            model_registry: self.model_registry.clone(),
            role_name: self.role_name.clone(),
            thinking_level: self.thinking_level,
            config: self.config.clone(),
            active_workflow_scope: self.active_workflow_scope.clone(),
            runtime_signal_tx: self.runtime_signal_tx.clone(),
            ui_tx,
            preloaded_lua_tools: self.preloaded_lua_tools(),
            prompt_context: Some(imp_core::builder::PromptContext::from_facts(Vec::new())),
            tui_trace: self.tui_trace.clone(),
        }
    }

    pub(super) fn start_agent_for_prompt_in_background(
        &mut self,
        text: String,
        agent_cwd: PathBuf,
    ) {
        let request = self.agent_start_request();
        let signal_tx = self.runtime_signal_tx.clone();
        self.trace_tui("agent_start_task queued");
        let start_task = tokio::spawn(async move {
            let mut build_task = tokio::task::spawn_blocking(move || {
                start_agent_from_request(request, &text, agent_cwd)
            });
            tokio::select! {
                _ = tokio::time::sleep(AGENT_START_STATUS_DELAY) => {
                    let _ = signal_tx
                        .send(RuntimeSignal::AgentStartStatus {
                            key: "startup".into(),
                            text: Some("indexing repo…".into()),
                        });
                }
                result = &mut build_task => {
                    let signal = agent_start_join_result_to_signal(result);
                    let _ = signal_tx.send(signal);
                    return;
                }
            }

            let signal = agent_start_join_result_to_signal(build_task.await);
            let _ = signal_tx.send(RuntimeSignal::AgentStartStatus {
                key: "startup".into(),
                text: None,
            });
            let _ = signal_tx.send(signal);
        });
        self.agent_start_task = Some(start_task);
    }

    pub(super) fn finish_agent_start(&mut self, result: AgentStartResult) {
        self.agent_start_task = None;
        self.status_items.remove("startup");
        self.agent_handle = Some(AgentHandle {
            event_rx: tokio::sync::mpsc::unbounded_channel().1,
            command_tx: result.command_tx,
            cancel_token: result.cancel_token,
        });
        self.agent_task = Some(result.task);
        self.agent_event_task = Some(result.event_task);
    }

    pub(super) fn fail_agent_start(&mut self, error: String) {
        self.agent_start_task = None;
        self.status_items.remove("startup");
        self.is_streaming = false;
        self.streaming_anchor_user_index = None;
        if self
            .messages
            .last()
            .is_some_and(|message| message.role == MessageRole::Assistant)
        {
            self.messages.pop();
        }
        self.messages.push(DisplayMessage {
            role: MessageRole::Error,
            content: error,
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: false,
            timestamp: imp_llm::now(),
        });
        self.invalidate_chat_render_cache();
        self.needs_redraw = true;
    }

    pub(super) fn try_prompt_command(&mut self, text: &str) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }

        if let Some(cmd) = trimmed.strip_prefix("!!") {
            self.run_shell_command(cmd.trim());
            return true;
        }

        if let Some(cmd) = trimmed.strip_prefix('!') {
            self.run_shell_command(cmd.trim());
            return true;
        }

        if let Some(cmd) = trimmed.strip_prefix(':') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                self.push_system_msg("Usage: :cd <path>, :pwd, :! <command>, or : <command>");
                return true;
            }
            if let Some(path) = cmd.strip_prefix("cd").and_then(command_arg) {
                self.change_working_directory(path);
                return true;
            }
            if cmd == "pwd" {
                self.push_system_msg(&self.cwd.display().to_string());
                return true;
            }
            let shell_cmd = cmd.strip_prefix('!').map(str::trim).unwrap_or(cmd);
            self.run_shell_command(shell_cmd);
            return true;
        }

        false
    }

    pub(super) fn change_working_directory(&mut self, path: &str) {
        if path.is_empty() {
            self.push_system_msg(&self.cwd.display().to_string());
            return;
        }
        let target = expand_prompt_path(path, &self.cwd);
        match target.canonicalize() {
            Ok(path) if path.is_dir() => {
                self.cwd = path;
                self.push_system_msg(&format!("cwd: {}", self.cwd.display()));
            }
            Ok(path) => self.push_error_msg(&format!("Not a directory: {}", path.display())),
            Err(error) => self.push_error_msg(&format!("cd failed: {error}")),
        }
    }

    pub(super) fn run_shell_command(&mut self, command: &str) {
        if command.is_empty() {
            self.push_system_msg("Usage: ! <command> or !! <command>");
            return;
        }
        match Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .output()
        {
            Ok(output) => {
                let mut text = format!("$ {command}\n");
                text.push_str(&String::from_utf8_lossy(&output.stdout));
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                if !output.status.success() {
                    text.push_str(&format!("\n(exit {})", output.status));
                }
                self.push_system_msg(text.trim_end());
            }
            Err(error) => self.push_error_msg(&format!("Shell command failed: {error}")),
        }
    }

    pub(super) fn queue_streaming_message(&mut self, message: QueuedMessage) {
        if let Some(previous) = self.message_queue.pop() {
            self.send_steering_message(previous.text().to_string());
        }
        self.message_queue.push(message);
        self.editor.clear();
        self.needs_redraw = true;
    }

    pub(super) fn send_steering_message(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: text.clone(),
            thinking: None,
            tool_calls: Vec::new(),
            assistant_blocks: Vec::new(),
            is_streaming: false,
            timestamp: imp_llm::now(),
        });
        self.invalidate_chat_render_cache();
        let _ = self.session.append(SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            message: imp_llm::Message::user(&text),
        });
        if let Some(ref handle) = self.agent_handle {
            let _ = handle.command_tx.try_send(AgentCommand::Steer(text));
        }
    }

    pub(super) fn send_message(&mut self) {
        let text = self.editor.content().to_string();
        if text.trim().is_empty() {
            return;
        }

        if self.try_prompt_command(&text) {
            self.editor.push_history();
            self.editor.clear();
            return;
        }
        // Check for slash commands. Only a single-line, slash-prefixed input is
        // treated as a command; pasted absolute paths or file contents can start
        // with `/` and must still be sent to the agent as normal text.
        if !text.contains('\n') {
            if let Some(cmd_text) = text.strip_prefix('/') {
                let typed = cmd_text.trim();
                let canonical_typed = if typed.eq_ignore_ascii_case("improve safe") {
                    "improve safe"
                } else {
                    typed
                };
                // Resolve prefix: exact match first, then unique prefix match.
                // Keep the original text for /skill:<name> so arguments survive.
                let commands = self.slash_commands();
                let cmd =
                    if canonical_typed == "improve safe" || canonical_typed.starts_with("skill:") {
                        canonical_typed.to_string()
                    } else {
                        let mut typed_parts = canonical_typed.splitn(2, char::is_whitespace);
                        let typed_command = typed_parts.next().unwrap_or_default();
                        let typed_args = typed_parts.next().unwrap_or_default().trim();
                        let resolved_command = commands
                            .iter()
                            .find(|c| c.name == typed_command)
                            .or_else(|| commands.iter().find(|c| c.name.starts_with(typed_command)))
                            .map(|c| c.name.as_str())
                            .unwrap_or(typed_command);
                        if typed_args.is_empty() {
                            resolved_command.to_string()
                        } else {
                            format!("{resolved_command} {typed_args}")
                        }
                    };
                self.execute_command(&cmd);
                self.editor.push_history();
                self.editor.clear();
                return;
            }
        }

        if self.compaction_task.is_some() || self.lua_command_task.is_some() {
            self.push_system_msg(
                "A background slash command is running; wait for it to finish before sending a new prompt.",
            );
            return;
        }

        // Send natural-language prompts directly; workflow profiles are not surfaced
        // as slash-command takeover UX in the TUI.
        // Add user message, assistant placeholder, and session entry before deferring agent start.
        self.enqueue_visible_agent_turn(text.clone());
        self.editor.push_history();
        self.editor.clear();
        self.needs_redraw = true;
    }

    pub(super) fn start_pending_agent_after_redraw(&mut self) {
        let Some(text) = self.pending_agent_prompt.take() else {
            return;
        };
        self.pending_agent_visible_text = None;
        let agent_cwd = self
            .pending_agent_cwd
            .take()
            .unwrap_or_else(|| self.cwd.clone());

        self.turn_tracker.start_now();
        self.agent_turn_started_at = Some(Instant::now());
        self.first_agent_event_seen = false;
        self.start_agent_for_prompt_in_background(text, agent_cwd);
    }

    pub(super) fn checkpoints_command(&mut self) {
        let records = self.session.checkpoint_records();
        if records.is_empty() {
            self.push_system_msg("No checkpoints recorded for this session.");
            return;
        }
        let mut lines = vec!["Recorded checkpoints:".to_string()];
        for record in records {
            let label = record
                .label
                .as_deref()
                .filter(|label| !label.trim().is_empty())
                .unwrap_or("unlabeled");
            lines.push(format!("- {} — {}", record.checkpoint_id, label));
        }
        self.push_system_msg(&lines.join("\n"));
    }

    pub(super) fn restore_checkpoint_command(&mut self, needle: &str) {
        match self.session.find_checkpoint_record(needle) {
            None => self.push_system_msg(&format!("Checkpoint not found: {needle}")),
            Some(record) => {
                let mut lines = vec![format!(
                    "Checkpoint `{}` is recorded for this session, but TUI restore is not wired yet.",
                    record.checkpoint_id
                )];
                if let Some(label) = record.label {
                    lines.push(format!("Label: {label}"));
                }
                if !record.files.is_empty() {
                    lines.push("Files:".into());
                    for path in record.files {
                        lines.push(format!("- {path}"));
                    }
                }
                self.push_system_msg(&lines.join("\n"));
            }
        }
    }

    pub(super) fn active_workflow_run_label(&self) -> Option<String> {
        self.active_workflow_run
            .as_ref()
            .map(|run| format!("run {} {}", run.run_id, run.status))
    }

    pub(super) fn active_workflow_scope_label(&self) -> Option<String> {
        self.active_workflow_scope.as_ref().map(|scope| {
            let mut title = scope.title.trim().to_string();
            const MAX_TITLE_CHARS: usize = 42;
            if title.chars().count() > MAX_TITLE_CHARS {
                title = title.chars().take(MAX_TITLE_CHARS).collect::<String>();
                title.push('…');
            }
            if title.is_empty() {
                format!("mana {}", scope.id)
            } else {
                format!("mana {} {}", scope.id, title)
            }
        })
    }

    #[cfg(feature = "mana-ui")]
    #[allow(dead_code)]
    pub(super) fn set_active_workflow_run(&mut self, id: &str) {
        let id = id.trim();
        if id.is_empty() {
            let Some(active_id) = self
                .active_workflow_run
                .as_ref()
                .map(|run| run.run_id.clone())
            else {
                self.push_system_msg("Usage: /run <run-id> or /run clear");
                return;
            };
            self.refresh_active_workflow_run(&active_id);
            return;
        }
        if id.eq_ignore_ascii_case("clear") || id.eq_ignore_ascii_case("none") {
            self.active_workflow_run = None;
            self.push_system_msg("Active workflow run cleared");
            return;
        }

        self.refresh_active_workflow_run(id);
    }

    #[cfg(feature = "mana-ui")]
    #[allow(dead_code)]
    pub(super) fn refresh_active_workflow_run(&mut self, id: &str) {
        match workflow_run_summary(id) {
            Ok(Some(summary)) => {
                self.push_system_msg(&format!(
                    "Active mana run: {} {} ({}/{}, failed {})",
                    summary.run_id,
                    summary.status,
                    summary.total_closed,
                    summary.total_units,
                    summary.total_failed
                ));
                self.active_workflow_run = Some(summary);
            }
            Ok(None) => self.push_system_msg(&format!("Could not find workflow run {id}")),
            Err(err) => self.push_system_msg(&format!("Could not read workflow run {id}: {err}")),
        }
    }

    pub(super) fn eval_candidate_command(&mut self, args: &str) {
        let Some(evidence_path) = self.status_items.get("evidence").map(PathBuf::from) else {
            self.push_warning_msg("No run evidence is available yet; run something before /eval.");
            return;
        };
        let Some(run_root) = evidence_path.parent().map(Path::to_path_buf) else {
            self.push_warning_msg("Could not infer run artifact directory from evidence path.");
            return;
        };
        let expected = command_option_value(args, "note")
            .map(|_| args.split("--note").next().unwrap_or(args).trim())
            .unwrap_or(args)
            .split("--verifier")
            .next()
            .unwrap_or(args)
            .trim();
        if expected.is_empty() {
            self.push_system_msg(
                "Usage: /eval <expected behavior> [--note correction] [--verifier command]",
            );
            return;
        }
        let note = command_option_value(args, "note");
        let verifier = command_option_value(args, "verifier");
        let run_id = run_root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "run".into());
        let candidate_id = format!("{run_id}-manual");
        let mut candidate =
            EvalCandidate::new(candidate_id.clone(), EvalFailureMode::UserCorrection);
        candidate.source.run_id = Some(run_id.clone());
        candidate.expected_behavior = EvalExpectedBehavior {
            summary: expected.to_string(),
            assertions: verifier
                .as_ref()
                .map(|command| vec![format!("{command} passes")])
                .unwrap_or_default(),
        };
        candidate.actual_behavior = note.map(|summary| EvalActualBehavior {
            summary,
            error_excerpt: None,
        });
        if let Some(command) = verifier {
            candidate.verifiers.push(EvalVerifier {
                name: "manual verifier".into(),
                command: Some(command),
                required: true,
                ..EvalVerifier::default()
            });
        }
        candidate.artifact_refs = vec![
            imp_core::eval_candidate::EvalArtifactRef {
                kind: "evidence".into(),
                path: evidence_path,
                summary: Some("Run evidence packet".into()),
                sha256: None,
            },
            imp_core::eval_candidate::EvalArtifactRef {
                kind: "trace".into(),
                path: run_root.join("trace.jsonl"),
                summary: Some("Structured runtime event trace".into()),
                sha256: None,
            },
        ];
        candidate.privacy = EvalPrivacy {
            redaction_status: EvalRedactionStatus::Unreviewed,
            redaction_rules: Vec::new(),
            contains_sensitive_data: false,
        };
        let candidate = redact_eval_candidate(candidate);
        let path = run_root
            .join("eval-candidates")
            .join(&candidate_id)
            .join("candidate.json");
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                self.push_error_msg(&format!("Could not create eval candidate directory: {err}"));
                return;
            }
        }
        match serde_json::to_string_pretty(&candidate)
            .map_err(std::io::Error::other)
            .and_then(|json| std::fs::write(&path, json))
        {
            Ok(()) => {
                self.status_items
                    .insert("eval-candidate".into(), path.display().to_string());
                self.push_system_msg(&format!("Saved eval candidate: {}", path.display()));
            }
            Err(err) => self.push_error_msg(&format!("Could not save eval candidate: {err}")),
        }
    }

    pub(super) fn execute_command(&mut self, cmd: &str) {
        let mut parts = cmd.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("").trim();

        match command {
            "quit" | "q" => {
                self.running = false;
            }
            "model" => {
                self.open_model_selector();
            }
            "tree" => {
                self.open_tree_view();
            }
            "new" => {
                self.messages.clear();
                self.invalidate_chat_render_cache();
                self.session = SessionManager::in_memory();
                self.tool_focus = None;
                self.tool_focus_pinned = false;
                self.sidebar_auto_follow = true;
                self.invalidate_chat_render_cache();
                self.accumulated_usage = Usage::default();
                self.accumulated_cost = Cost::default();
                self.current_context_tokens = 0;
            }
            "compact" => {
                self.run_manual_compaction();
            }
            "hotkeys" => {
                self.push_system_msg(
                    "Keyboard shortcuts:\n\
  Enter         Send message\n\
  Shift+Enter   New line\n\
  Alt+Enter     Queue follow-up while streaming\n\
  Ctrl+C        Clear / Abort / Quit\n\
  Cmd+C         Copy selection\n\
  Ctrl+V/Cmd+V  Paste clipboard\n\
  Ctrl+L        Model selector\n\
  Ctrl+P        Next chosen model\n\
  Ctrl+Shift+P  Previous chosen model\n\
  Tab           Show/hide sidebar\n\
  Ctrl+O        Open selected read file in editor\n\
  Ctrl+Up/Down  Focus previous/next tool\n\
  Shift+Tab     Cycle thinking level\n\
  @             File finder\n\
  /command      Slash commands\n\
  ! <cmd>       Run shell command in current cwd\n\
  !! <cmd>      Run shell command without adding output to agent context\n\
  :cd <path>    Change working directory\n\
  :pwd          Show working directory\n\
  : <cmd>       Run shell command\n\
  Esc           Cancel current turn; when idle, clear pending prompt/loop
  /stop         Stop active work and clear pending prompt/loop
  PageUp/Down   Scroll",
                );
            }
            "loop" => self.start_loop_command(args),
            "stop" => self.stop_active_work(),
            "settings" => {
                self.open_settings();
            }
            "resume" => {
                self.start_session_list_load();
            }
            "name" => {
                let new_name = cmd.strip_prefix("name").unwrap_or("").trim();
                if new_name.is_empty() {
                    self.push_system_msg("Usage: /name <session name>");
                } else {
                    self.session.set_name(new_name);
                    self.push_system_msg(&format!("Session renamed to: {new_name}"));
                }
            }
            "reload" => {
                match imp_core::config::Config::resolve(
                    &imp_core::config::Config::user_config_dir(),
                    Some(&self.cwd),
                ) {
                    Ok(new_config) => {
                        self.config = new_config;
                        // Reload Lua extensions
                        self.reload_lua_extensions();
                        self.push_system_msg("Config and Lua extensions reloaded.");
                    }
                    Err(e) => self.push_system_msg(&format!("Reload failed: {e}")),
                }
            }
            "help" => {
                self.push_system_msg(concat!(
                    "Commands:\n",
                    "  /new        — start fresh session\n",
                    "  /resume     — resume/search sessions\n",
                    "  /model      — switch model\n",
                    "  /compact    — compress context\n",
                    "  /quit       — exit\n",
                    "  /loop [msg] — continue or auto-loop current intent\n",
                    "  /stop       — stop active work/loop\n",
                    "  /reload     — reload config and Lua extensions\n",
                    "  /setup      — run setup wizard\n",
                    "  /secrets [provider] — save/list API keys & service secrets\n",
                    "  /login [provider]   — OAuth login (Anthropic/OpenAI/Kimi Code)\n",
                    "  /name <n>   — rename session\n",
                    "  /tree       — session tree\n",
                    "  /settings   — edit settings\n",
                    "  /help       — this message\n",
                    "  :cd <path>  — change working directory\n",
                    "  :pwd        — show working directory\n",
                    "  : <cmd>     — run shell command\n",
                    "  ! <cmd>     — run shell command\n",
                    "  !! <cmd>    — run shell command without adding output to agent context\n",
                    "\nTools: web.read supports web pages and public YouTube URLs (metadata + captions when available).",
                ));
            }
            "login" => {
                if let Some(provider) = cmd.split_whitespace().nth(1) {
                    self.start_login(provider);
                } else {
                    self.open_login_picker();
                }
            }
            "secrets" => {
                if let Some(provider) = cmd.split_whitespace().nth(1) {
                    self.start_secrets_flow(provider);
                } else {
                    self.open_secrets_picker();
                }
            }
            "welcome" | "setup" => {
                let all_models = self.model_registry.list().to_vec();
                self.mode = UiMode::Welcome(WelcomeState::new(&all_models));
            }
            "memory" | "mem" => self.handle_memory_command(cmd),
            "checkpoints" => self.checkpoints_command(),
            "restore" | "restore-checkpoint" => self.restore_checkpoint_command(args),
            "eval" => self.eval_candidate_command(args),
            _ => {
                // Try Lua extension commands before reporting unknown
                if !self.try_lua_command(cmd) && !self.try_skill_command(cmd) {
                    self.messages.push(DisplayMessage {
                        role: MessageRole::Error,
                        content: format!("Unknown command: /{cmd}"),
                        thinking: None,
                        tool_calls: Vec::new(),
                        assistant_blocks: Vec::new(),
                        is_streaming: false,
                        timestamp: imp_llm::now(),
                    });
                }
            }
        }
        self.editor.clear();
    }

    /// Handle `/memory` subcommands.
    ///
    /// - `/memory`           — show both stores
    /// - `/memory add <t>`   — add entry to memory.md
    /// - `/memory user <t>`  — add entry to user.md
    /// - `/memory remove <t>` — remove matching entry from memory.md
    /// - `/memory remove user <t>` — remove matching entry from user.md
    /// - `/memory clear`     — wipe memory.md
    /// - `/memory clear user` — wipe user.md
    pub(super) fn handle_memory_command(&mut self, cmd: &str) {
        use imp_core::memory::MemoryStore;

        let config_dir = Config::user_config_dir();
        let mem_path = config_dir.join("memory.md");
        let user_path = config_dir.join("user.md");
        let mem_limit = self.config.learning.memory_char_limit;
        let user_limit = self.config.learning.user_char_limit;

        // Strip the command name prefix ("memory" or "mem") to get arguments
        let rest = cmd
            .strip_prefix("memory")
            .or_else(|| cmd.strip_prefix("mem"))
            .unwrap_or("")
            .trim();

        if rest.is_empty() {
            // Show both stores
            let mut output = String::new();

            match MemoryStore::load(&mem_path, mem_limit) {
                Ok(store) => {
                    let (used, limit) = store.usage();
                    output.push_str(&format!("Memory ({used}/{limit} chars):\n"));
                    if store.entries().is_empty() {
                        output.push_str("  (empty)\n");
                    } else {
                        for (i, entry) in store.entries().iter().enumerate() {
                            output.push_str(&format!("  {}. {}\n", i + 1, entry));
                        }
                    }
                }
                Err(e) => output.push_str(&format!("Error loading memory.md: {e}\n")),
            }

            output.push('\n');

            match MemoryStore::load(&user_path, user_limit) {
                Ok(store) => {
                    let (used, limit) = store.usage();
                    output.push_str(&format!("User profile ({used}/{limit} chars):\n"));
                    if store.entries().is_empty() {
                        output.push_str("  (empty)\n");
                    } else {
                        for (i, entry) in store.entries().iter().enumerate() {
                            output.push_str(&format!("  {}. {}\n", i + 1, entry));
                        }
                    }
                }
                Err(e) => output.push_str(&format!("Error loading user.md: {e}\n")),
            }

            if !self.config.learning.enabled {
                output.push_str("\n⚠ Learning is disabled in config. Memory won't be loaded into the system prompt.");
            }

            self.push_system_msg(output.trim_end());
            return;
        }

        let mut words = rest.splitn(2, char::is_whitespace);
        let sub = words.next().unwrap_or("");
        let arg = words.next().unwrap_or("").trim();

        match sub {
            "add" => {
                if arg.is_empty() {
                    self.push_system_msg("Usage: /memory add <text>");
                    return;
                }
                match MemoryStore::load(&mem_path, mem_limit) {
                    Ok(mut store) => match store.add(arg) {
                        Ok(result) => {
                            self.push_system_msg(&format!("{} [{}]", result.message, result.usage))
                        }
                        Err(e) => self.push_system_msg(&format!("Error: {e}")),
                    },
                    Err(e) => self.push_system_msg(&format!("Error: {e}")),
                }
            }
            "user" => {
                if arg.is_empty() {
                    self.push_system_msg("Usage: /memory user <text>");
                    return;
                }
                match MemoryStore::load(&user_path, user_limit) {
                    Ok(mut store) => match store.add(arg) {
                        Ok(result) => {
                            self.push_system_msg(&format!("{} [{}]", result.message, result.usage))
                        }
                        Err(e) => self.push_system_msg(&format!("Error: {e}")),
                    },
                    Err(e) => self.push_system_msg(&format!("Error: {e}")),
                }
            }
            "remove" | "rm" => {
                if arg.is_empty() {
                    self.push_system_msg("Usage: /memory remove <text>");
                    return;
                }
                // Check if removing from user store: "/memory remove user <text>"
                if let Some(user_arg) = arg.strip_prefix("user ").map(|s| s.trim()) {
                    if user_arg.is_empty() {
                        self.push_system_msg("Usage: /memory remove user <text>");
                        return;
                    }
                    match MemoryStore::load(&user_path, user_limit) {
                        Ok(mut store) => match store.remove(user_arg) {
                            Ok(result) => self
                                .push_system_msg(&format!("{} [{}]", result.message, result.usage)),
                            Err(e) => self.push_system_msg(&format!("Error: {e}")),
                        },
                        Err(e) => self.push_system_msg(&format!("Error: {e}")),
                    }
                } else {
                    match MemoryStore::load(&mem_path, mem_limit) {
                        Ok(mut store) => match store.remove(arg) {
                            Ok(result) => self
                                .push_system_msg(&format!("{} [{}]", result.message, result.usage)),
                            Err(e) => self.push_system_msg(&format!("Error: {e}")),
                        },
                        Err(e) => self.push_system_msg(&format!("Error: {e}")),
                    }
                }
            }
            "replace" => {
                // "/memory replace <old> -> <new>"
                if let Some((old, new)) = arg.split_once("->") {
                    let old = old.trim();
                    let new = new.trim();
                    if old.is_empty() || new.is_empty() {
                        self.push_system_msg("Usage: /memory replace <old text> -> <new text>");
                        return;
                    }
                    match MemoryStore::load(&mem_path, mem_limit) {
                        Ok(mut store) => match store.replace(old, new) {
                            Ok(result) => self
                                .push_system_msg(&format!("{} [{}]", result.message, result.usage)),
                            Err(e) => self.push_system_msg(&format!("Error: {e}")),
                        },
                        Err(e) => self.push_system_msg(&format!("Error: {e}")),
                    }
                } else {
                    self.push_system_msg("Usage: /memory replace <old text> -> <new text>");
                }
            }
            "clear" => {
                let target = arg;
                if target == "user" {
                    if user_path.exists() {
                        match std::fs::write(&user_path, "") {
                            Ok(_) => self.push_system_msg("User profile cleared."),
                            Err(e) => self.push_system_msg(&format!("Error: {e}")),
                        }
                    } else {
                        self.push_system_msg("User profile is already empty.");
                    }
                } else if target.is_empty() {
                    if mem_path.exists() {
                        match std::fs::write(&mem_path, "") {
                            Ok(_) => self.push_system_msg("Memory cleared."),
                            Err(e) => self.push_system_msg(&format!("Error: {e}")),
                        }
                    } else {
                        self.push_system_msg("Memory is already empty.");
                    }
                } else {
                    self.push_system_msg("Usage: /memory clear [user]");
                }
            }
            "help" => {
                self.push_system_msg(concat!(
                    "Memory commands:\n",
                    "  /memory              — show all entries\n",
                    "  /memory add <text>   — add to memory\n",
                    "  /memory user <text>  — add to user profile\n",
                    "  /memory remove <text>  — remove from memory\n",
                    "  /memory remove user <text> — remove from user profile\n",
                    "  /memory replace <old> -> <new> — replace entry\n",
                    "  /memory clear        — clear memory\n",
                    "  /memory clear user   — clear user profile",
                ));
            }
            _ => {
                self.push_system_msg(&format!(
                    "Unknown memory subcommand: {sub}\nUse /memory help for usage."
                ));
            }
        }
    }

    pub(super) fn slash_commands(&self) -> Vec<crate::views::command_palette::SlashCommand> {
        let extension_commands = self
            .lua_runtime
            .as_ref()
            .and_then(|runtime| runtime.lock().ok().map(|guard| guard.command_summaries()))
            .unwrap_or_default();
        let commands = merge_extension_commands(builtin_commands(), extension_commands);
        merge_skill_commands(commands, self.skill_summaries())
    }

    pub(super) fn skill_summaries(&self) -> Vec<(String, String)> {
        self.startup_surface_metadata
            .skills
            .iter()
            .map(|skill| (skill.name.clone(), skill.description.clone()))
            .collect()
    }

    pub(super) fn try_skill_command(&mut self, cmd: &str) -> bool {
        let (skill_name, args) = if let Some(rest) = cmd.strip_prefix("skill:") {
            let skill_name = rest.split_whitespace().next().unwrap_or("");
            let args = rest.strip_prefix(skill_name).unwrap_or("").trim();
            (skill_name, args)
        } else {
            let skill_name = cmd.split_whitespace().next().unwrap_or("");
            let args = cmd.strip_prefix(skill_name).unwrap_or("").trim();
            (skill_name, args)
        };

        if skill_name.is_empty() {
            return false;
        }

        let Some(skill) = self
            .startup_surface_metadata
            .skills
            .iter()
            .find(|skill| skill.name == skill_name)
            .cloned()
        else {
            return false;
        };

        let content = match std::fs::read_to_string(&skill.path) {
            Ok(content) => content,
            Err(error) => {
                self.push_error_msg(&format!("Failed to load skill `{skill_name}`: {error}"));
                return true;
            }
        };

        let prompt = imp_core::resources::render_skill_invocation(skill_name, &content, args);
        self.editor.set_content(&prompt);
        self.send_message();
        true
    }

    /// Reload Lua extensions: re-scan directories, re-create runtime, and update
    /// the stored runtime handle. Tools are not re-registered on the running
    /// agent (only new agents will pick them up), but commands become available
    /// immediately.
    pub(super) fn reload_lua_extensions(&mut self) {
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

    pub(super) fn lua_command_call_context(&self) -> imp_lua::LuaCallContext {
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
    pub(super) fn try_lua_command(&mut self, cmd: &str) -> bool {
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

    pub(super) fn restart_after_lua_command(&mut self) {
        match std::env::current_exe() {
            Ok(exe) => match std::process::Command::new(&exe).spawn() {
                Ok(_) => {
                    self.push_system_msg("Restarting imp into the updated binary…");
                    self.running = false;
                }
                Err(error) => {
                    self.push_error_msg(&format!(
                        "Restart requested, but failed to launch {}: {error}",
                        exe.display()
                    ));
                }
            },
            Err(error) => {
                self.push_error_msg(&format!(
                    "Restart requested, but failed to resolve current imp executable: {error}"
                ));
            }
        }
    }

    pub(super) fn start_secrets_flow(&mut self, provider: &str) {
        self.mode = UiMode::Normal;
        self.secrets_flow = Some(SecretsFlowState::AwaitingFieldNames {
            provider: provider.to_string(),
        });
        let (tx, _rx) = tokio::sync::oneshot::channel();
        self.begin_ask(
            crate::views::ask_bar::AskState::new(
                format!(
                    "{}\n\nField names (comma-separated) [api_key]:",
                    prompt_text_for_secret_provider(provider)
                ),
                String::new(),
                vec![],
                false,
            ),
            AskReply::Input(tx),
        );
    }

    pub(super) fn start_login(&mut self, provider: &str) {
        if !oauth_provider(provider) {
            self.push_error_msg(&format!(
                "/login {provider} is OAuth-only. Use /secrets {provider} for API keys/secrets."
            ));
            return;
        }

        let status_message = match provider {
            "anthropic" => "Opening browser for Anthropic login...",
            "openai" | "openai-codex" => "Opening browser for OpenAI / ChatGPT login...",
            "kimi-code" => "Opening browser for Kimi Code login...",
            _ => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!(
                        "OAuth login for '{provider}' not supported. Use /secrets {provider} for API keys."
                    ),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
                return;
            }
        };

        let in_welcome = matches!(self.mode, UiMode::Welcome(_));
        if !in_welcome {
            self.mode = UiMode::Normal;
            self.push_system_msg(status_message);
        }

        let auth_path = imp_core::storage::global_auth_path();
        let provider = provider.to_string();
        let signal_tx = self.runtime_signal_tx.clone();
        let task = tokio::spawn(async move {
            let provider_for_url = provider.clone();
            let url_signal_tx = signal_tx.clone();
            let emit_url = move |url: &str| {
                let browser_opened = open_url(url);
                let _ = url_signal_tx.send(RuntimeSignal::LoginUrlReady {
                    provider: provider_for_url.clone(),
                    url: url.to_string(),
                    browser_opened,
                });
            };
            let login_result = match provider.as_str() {
                "anthropic" => {
                    imp_llm::oauth::anthropic::AnthropicOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            || async { None },
                        )
                        .await
                }
                "openai" | "openai-codex" => {
                    imp_llm::oauth::chatgpt::ChatGptOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            || async { None },
                        )
                        .await
                }
                "kimi-code" => {
                    imp_llm::oauth::kimi_code::KimiCodeOAuth::new()
                        .login(
                            |url| {
                                emit_url(url);
                            },
                            |_msg| {
                                // Messages are silently dropped in the TUI background task;
                                // the browser URL is the primary signal.
                            },
                        )
                        .await
                }
                _ => unreachable!(),
            };

            match login_result {
                Ok(credential) => {
                    let success_message = imp_llm::auth::oauth_display_info_for_credential(
                        provider.as_str(),
                        &credential,
                    )
                    .map(|info| info.login_message(provider.as_str()))
                    .unwrap_or_else(|| format!("Logged in to {} successfully.", provider));

                    let mut store = AuthStore::load(&auth_path)
                        .unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
                    match provider.as_str() {
                        "anthropic" => {
                            let _ = store.store(
                                "anthropic",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        "openai" | "openai-codex" => {
                            let _ = store.store(
                                "openai",
                                imp_llm::auth::StoredCredential::OAuth(credential.clone()),
                            );
                            let _ = store.store(
                                "openai-codex",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        "kimi-code" => {
                            let _ = store.store(
                                "kimi-code",
                                imp_llm::auth::StoredCredential::OAuth(credential),
                            );
                        }
                        _ => {}
                    }
                    LoginTaskExit::Success(success_message)
                }
                Err(e) => LoginTaskExit::Failed(format!("OAuth login failed: {e}")),
            }
        });
        self.login_task = Some(task);
    }

    pub(super) fn open_secrets_picker(&mut self) {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let providers = secret_providers(&ProviderRegistry::with_builtins())
            .into_iter()
            .map(|mut provider| {
                provider.configured = provider_logged_in(&auth_store, &provider.id);
                provider
            })
            .collect();
        self.mode = UiMode::SecretsPicker(SecretsPickerState::new(providers));
    }

    pub(super) fn open_login_picker(&mut self) {
        let auth_path = imp_core::storage::global_auth_path();
        let auth_store =
            AuthStore::load(&auth_path).unwrap_or_else(|_| AuthStore::new(auth_path.clone()));
        let providers = login_providers(&ProviderRegistry::with_builtins())
            .into_iter()
            .filter(|provider| oauth_provider(provider.id))
            .map(|mut provider| {
                provider.logged_in = provider_logged_in(&auth_store, provider.id);
                provider
            })
            .collect();
        self.mode = UiMode::LoginPicker(LoginPickerState::new(providers));
    }
}
