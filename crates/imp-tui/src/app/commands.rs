use std::process::Command;

use super::*;

mod auth;
mod lua;
mod workflow_label;

use workflow_label::workflow_prompt_bar_label_for_run;

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
        if !matches!(
            self.config.context.auto_compaction.mode,
            imp_core::config::AutoCompactionMode::Disabled
        ) {
            if let Some(meta) = self.model_registry.resolve_meta(&self.model_name, None) {
                let trigger = self.config.context.auto_compaction.trigger_ratio;
                let usage = imp_core::context::context_usage_for_meta(
                    &self.session.get_active_messages(),
                    &meta,
                );
                match imp_core::compaction::activate_checkpoint_for_usage(
                    &mut self.session,
                    usage.used,
                    usage.limit,
                    trigger,
                ) {
                    Ok(Some(result)) => {
                        self.current_context_tokens = result.tokens_after;
                    }
                    Ok(None) => {}
                    Err(error) => self.push_warning_msg(&format!(
                        "Automatic compaction was not activated: {error}. Continuing with unchanged context."
                    )),
                }
            }
        }
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
        self.agent_task_state = Some(result.task_state);
        self.agent_task = Some(result.task);
        self.agent_event_task = Some(result.event_task);
    }

    pub(super) fn fail_agent_start(&mut self, error: String) {
        self.agent_start_task = None;
        self.status_items.remove("startup");
        self.agent_task_state = None;
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
        if let Err(error) = self.session.append(SessionEntry::Message {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            message: imp_llm::Message::user(&text),
        }) {
            self.report_session_persist_error("steering message", error);
            return;
        }
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
                let mut typed_parts = canonical_typed.splitn(2, char::is_whitespace);
                let typed_command = typed_parts.next().unwrap_or_default();
                if typed_command == "plan" {
                    self.push_error_msg(&format!("Unknown command: /{canonical_typed}"));
                    self.editor.push_history();
                    self.editor.clear();
                    return;
                }
                if !canonical_typed.starts_with("skill:")
                    && typed_command != "improve"
                    && !commands.iter().any(|command| {
                        command.name == typed_command || command.name.starts_with(typed_command)
                    })
                {
                    self.execute_command(canonical_typed);
                    self.editor.push_history();
                    self.editor.clear();
                    return;
                }
                let cmd =
                    if canonical_typed == "improve safe" || canonical_typed.starts_with("skill:") {
                        canonical_typed.to_string()
                    } else {
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
            .map(workflow_prompt_bar_label_for_run)
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
                self.run_manual_compaction(matches!(args, "summarize" | "summary"));
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
                if args.is_empty() {
                    self.start_session_list_load();
                } else {
                    self.start_session_open(PathBuf::from(args));
                }
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
            "x" | "ext" | "lua" => self.handle_extension_command(args),
            "skill" | "s" => self.handle_skill_namespace_command(args),
            "workflow" | "w" => self.handle_workflow_command(args),
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
                    "  /x <cmd>    — run a Lua extension command; aliases: /ext, /lua\n",
                    "  /skill <n>  — run a skill; alias: /s\n",
                    "  /workflow <id> — select active workflow scope; alias: /w\n",
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

    pub(super) fn slash_commands(&self) -> Vec<crate::views::command_palette::SlashCommand> {
        let extension_commands = self.lua_command_summaries();
        let commands = merge_extension_commands(builtin_commands(), extension_commands);
        let commands = merge_skill_commands(commands, self.skill_summaries());
        crate::views::command_palette::merge_workflow_commands(commands, self.workflow_summaries())
    }

    pub(super) fn lua_command_summaries(&self) -> Vec<(String, String)> {
        self.lua_runtime
            .as_ref()
            .and_then(|runtime| runtime.lock().ok().map(|guard| guard.command_summaries()))
            .unwrap_or_default()
    }

    pub(super) fn workflow_summaries(&self) -> Vec<(String, String)> {
        self.startup_surface_metadata
            .workflows
            .iter()
            .map(|workflow| (workflow.id.clone(), workflow.title.clone()))
            .collect()
    }

    pub(super) fn skill_summaries(&self) -> Vec<(String, String)> {
        self.startup_surface_metadata
            .skills
            .iter()
            .map(|skill| (skill.name.clone(), skill.description.clone()))
            .collect()
    }

    pub(super) fn handle_extension_command(&mut self, args: &str) {
        let args = args.trim();
        if args.is_empty() {
            let summaries = self.lua_command_summaries();
            if summaries.is_empty() {
                self.push_system_msg("No Lua extension commands are loaded.");
                return;
            }
            let mut output = String::from("Lua extension commands:\n");
            for (name, description) in summaries {
                if description.trim().is_empty() {
                    output.push_str(&format!("  /x {name}\n"));
                } else {
                    output.push_str(&format!("  /x {name} — {description}\n"));
                }
            }
            self.push_system_msg(output.trim_end());
            return;
        }

        if !self.try_lua_command(args) {
            let command = args.split_whitespace().next().unwrap_or(args);
            self.push_error_msg(&format!("Unknown extension command: /x {command}"));
        }
    }

    pub(super) fn handle_skill_namespace_command(&mut self, args: &str) {
        let args = args.trim();
        if args.is_empty() {
            let summaries = self.skill_summaries();
            if summaries.is_empty() {
                self.push_system_msg("No skills are loaded.");
                return;
            }
            let mut output = String::from("Skills:\n");
            for (name, description) in summaries {
                if description.trim().is_empty() {
                    output.push_str(&format!("  /skill {name}\n"));
                } else {
                    output.push_str(&format!("  /skill {name} — {description}\n"));
                }
            }
            self.push_system_msg(output.trim_end());
            return;
        }

        if !self.try_skill_command(args) {
            let skill = args.split_whitespace().next().unwrap_or(args);
            self.push_error_msg(&format!("Unknown skill: /skill {skill}"));
        }
    }

    pub(super) fn handle_workflow_command(&mut self, args: &str) {
        let args = args.trim();
        if args.is_empty() {
            if self.startup_surface_metadata.workflows.is_empty() {
                self.push_system_msg("No workflows were discovered under .imp/workflows.");
                return;
            }
            let mut output = String::from("Workflows:\n");
            for workflow in &self.startup_surface_metadata.workflows {
                output.push_str(&format!(
                    "  /workflow {} — {} [{}]\n",
                    workflow.id, workflow.title, workflow.status
                ));
            }
            output.push_str("  /workflow clear — clear active workflow scope");
            self.push_system_msg(&output);
            return;
        }

        let workflow_id = args.split_whitespace().next().unwrap_or(args);
        if matches!(workflow_id, "clear" | "off" | "none") {
            self.active_workflow_scope = None;
            self.push_system_msg("Active workflow scope cleared.");
            return;
        }

        let Some(workflow) = self
            .startup_surface_metadata
            .workflows
            .iter()
            .find(|workflow| workflow.id == workflow_id)
            .cloned()
        else {
            self.push_error_msg(&format!("Unknown workflow: /workflow {workflow_id}"));
            return;
        };

        self.active_workflow_scope = Some(WorkflowUnitRef::new(
            workflow.id.clone(),
            workflow.title.clone(),
            Some(workflow.kind.clone()),
        ));
        self.push_system_msg(&format!(
            "Active workflow scope: {} — {}",
            workflow.id, workflow.title
        ));
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
}
