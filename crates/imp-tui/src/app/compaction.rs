use imp_core::compaction::activate_checkpoint;
use imp_core::compaction::checkpoint::CheckpointStore;

use crate::views::chat::{DisplayMessage, MessageRole};

use super::App;

impl App {
    pub(super) fn run_manual_compaction(&mut self, _summarize: bool) {
        if self.is_streaming {
            self.push_error_msg("Cannot compact while the agent is actively streaming.");
            return;
        }
        if self.checkpoint_task.is_some() {
            self.push_system_msg("A compaction checkpoint is still being prepared.");
            return;
        }
        self.finish_manual_compaction(String::new());
    }

    pub(super) fn finish_compaction_status_message(&mut self, content: &str) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == MessageRole::Compaction && message.is_streaming)
        {
            message.content = content.to_string();
            message.is_streaming = false;
            self.invalidate_chat_render_cache();
        }
    }

    pub(super) fn finish_lua_command_status_message(&mut self, content: &str) {
        self.finish_compaction_status_message(content);
    }

    pub(super) fn finish_manual_compaction(&mut self, _summary: String) {
        let Some(session_path) = self.session.path() else {
            self.push_error_msg("Compaction requires a durable session checkpoint.");
            return;
        };
        let store = CheckpointStore::for_session(session_path);
        let checkpoint = match store.load() {
            Ok(Some(checkpoint)) => checkpoint,
            Ok(None) => {
                self.push_error_msg(
                    "No validated compaction checkpoint is ready. Context was left unchanged.",
                );
                return;
            }
            Err(error) => {
                self.push_error_msg(&format!(
                    "Compaction checkpoint is invalid: {error}. Context was left unchanged."
                ));
                return;
            }
        };
        match activate_checkpoint(&mut self.session, &checkpoint) {
            Ok(compaction) => {
                self.current_context_tokens = compaction.tokens_after;
                self.load_session_messages();
                self.messages.push(DisplayMessage {
                    role: MessageRole::Compaction,
                    content: format!(
                        "Context compacted from validated checkpoint. Saved ~{} tokens.",
                        compaction
                            .tokens_before
                            .saturating_sub(compaction.tokens_after)
                    ),
                    thinking: None,
                    tool_calls: Vec::new(),
                    assistant_blocks: Vec::new(),
                    is_streaming: false,
                    timestamp: imp_llm::now(),
                });
            }
            Err(error) => self.push_error_msg(&format!(
                "Compaction failed: {error}. Context was left unchanged."
            )),
        }
    }
}
