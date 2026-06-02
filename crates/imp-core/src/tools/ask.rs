use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::error::Result;
use crate::ui::{SelectOption, SelectionAnswer, UserInterface};

pub struct AskTool;

#[async_trait]
impl Tool for AskTool {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn label(&self) -> &str {
        "Ask User"
    }
    fn description(&self) -> &str {
        "Ask the user a question. Use choices for single- or multi-select questions."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "choices": {
                    "type": "array",
                    "description": "Choices the user can select from. Use [\"Yes\", \"No\"] for yes/no questions.",
                    "items": { "type": "string" }
                },
                "multi_select": { "type": "boolean" },
                "allow_other": { "type": "boolean" },
                "placeholder": { "type": "string" }
            },
            "required": ["question"]
        })
    }
    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        if !ctx.ui.has_ui() {
            return Ok(ToolOutput::error(
                "Cannot access ask_user tool in this mode. Proceed with an explicit assumption if low-risk, or record a blocker/decision if consequential.",
            ));
        }

        let Some(question) = params["question"]
            .as_str()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        else {
            return Ok(ToolOutput::error("Missing required parameter: question"));
        };

        let choices = match parse_choices(&params) {
            Ok(choices) => choices,
            Err(message) => return Ok(ToolOutput::error(message)),
        };
        let multi_select = params["multi_select"].as_bool().unwrap_or(false);
        let allow_other = params["allow_other"].as_bool().unwrap_or(false);
        let placeholder = params["placeholder"].as_str().unwrap_or("");

        match choices {
            Some(mut choices) => {
                if allow_other {
                    choices.push(SelectOption {
                        label: "Other...".to_string(),
                        description: Some("Type a custom answer".to_string()),
                    });
                }

                if multi_select {
                    execute_multi_select(&*ctx.ui, question, placeholder, &choices, allow_other)
                        .await
                } else {
                    execute_single_select(&*ctx.ui, question, placeholder, &choices, allow_other)
                        .await
                }
            }
            None => match ctx.ui.input(question, placeholder).await {
                Some(answer) => Ok(answer_output(answer, true)),
                None => Ok(skipped_output(false)),
            },
        }
    }
}

fn parse_choices(
    params: &serde_json::Value,
) -> std::result::Result<Option<Vec<SelectOption>>, String> {
    let Some(value) = params.get("choices") else {
        return Ok(None);
    };
    let Some(values) = value.as_array() else {
        return Err("choices must be an array of strings".to_string());
    };
    if values.is_empty() {
        return Err("choices must not be empty".to_string());
    }
    if values.len() > 50 {
        return Err("choices must contain at most 50 items".to_string());
    }

    let mut seen = HashSet::new();
    let mut choices = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let Some(label) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(format!("choices[{index}] must be a non-empty string"));
        };
        if label.len() > 200 {
            return Err(format!("choices[{index}] is too long"));
        }
        if !seen.insert(label.to_string()) {
            return Err(format!("duplicate choice: {label}"));
        }
        choices.push(SelectOption {
            label: label.to_string(),
            description: None,
        });
    }

    Ok(Some(choices))
}

async fn execute_single_select(
    ui: &dyn UserInterface,
    question: &str,
    placeholder: &str,
    choices: &[SelectOption],
    allow_other: bool,
) -> Result<ToolOutput> {
    match ui
        .select_or_input_with_context(question, "", choices, placeholder)
        .await
    {
        Some(SelectionAnswer::Text(answer)) if allow_other => Ok(tool_text_with_details(
            &answer,
            json!({
                "answered": true,
                "skipped": false,
                "answer": answer,
                "answers": [answer],
                "other": true,
                "multi_select": false
            }),
        )),
        Some(SelectionAnswer::Text(answer)) => Ok(answer_output(answer, true)),
        Some(SelectionAnswer::Choice(index)) if allow_other && index == choices.len() - 1 => {
            match ui.input("Enter your answer:", placeholder).await {
                Some(answer) => Ok(tool_text_with_details(
                    &answer,
                    json!({
                        "answered": true,
                        "skipped": false,
                        "answer": answer,
                        "answers": [answer],
                        "other": true,
                        "multi_select": false
                    }),
                )),
                None => Ok(skipped_output(false)),
            }
        }
        Some(SelectionAnswer::Choice(index)) if index < choices.len() => {
            let answer = choices[index].label.clone();
            Ok(tool_text_with_details(
                &answer,
                json!({
                    "answered": true,
                    "skipped": false,
                    "answer": answer,
                    "answers": [answer],
                    "choice_index": index,
                    "choice_indices": [index],
                    "other": false,
                    "multi_select": false
                }),
            ))
        }
        _ => Ok(skipped_output(false)),
    }
}

async fn execute_multi_select(
    ui: &dyn UserInterface,
    question: &str,
    placeholder: &str,
    choices: &[SelectOption],
    allow_other: bool,
) -> Result<ToolOutput> {
    let Some(indices) = ui.multi_select_with_context(question, "", choices).await else {
        return Ok(skipped_output(true));
    };
    if indices.is_empty() {
        return Ok(skipped_output(true));
    }

    let other_index = choices.len().saturating_sub(1);
    let mut answers = Vec::new();
    let mut choice_indices = Vec::new();
    let mut other = false;
    for index in indices {
        if index >= choices.len() {
            continue;
        }
        if allow_other && index == other_index {
            other = true;
            if let Some(answer) = ui.input("Enter your answer:", placeholder).await {
                answers.push(answer);
                choice_indices.push(index);
            }
        } else {
            answers.push(choices[index].label.clone());
            choice_indices.push(index);
        }
    }

    if answers.is_empty() {
        return Ok(skipped_output(true));
    }

    let text = answers.join(", ");
    Ok(tool_text_with_details(
        &text,
        json!({
            "answered": true,
            "skipped": false,
            "answer": text,
            "answers": answers,
            "choice_indices": choice_indices,
            "other": other,
            "multi_select": true
        }),
    ))
}

fn tool_text_with_details(text: &str, details: serde_json::Value) -> ToolOutput {
    let mut output = ToolOutput::text(text);
    output.details = details;
    output
}

fn answer_output(answer: String, free_text: bool) -> ToolOutput {
    tool_text_with_details(
        &answer,
        json!({
            "answered": true,
            "skipped": false,
            "answer": answer,
            "answers": [answer],
            "free_text": free_text,
            "multi_select": false
        }),
    )
}

fn skipped_output(multi_select: bool) -> ToolOutput {
    tool_text_with_details(
        "User skipped",
        json!({
            "answered": false,
            "skipped": true,
            "multi_select": multi_select
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;
    use crate::ui::NullInterface;
    use std::sync::{Arc, Mutex};

    fn test_ctx<T: crate::ui::UserInterface + 'static>(ui: Arc<T>) -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
        ToolContext {
            cwd: std::path::PathBuf::from("/tmp"),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_tx: tx,
            command_tx: cmd_tx,
            ui: ui as Arc<dyn crate::ui::UserInterface>,
            file_cache: Arc::new(crate::tools::FileCache::new()),
            checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
            anchor_store: Arc::new(crate::tools::AnchorStore::new()),
            lua_tool_loader: None,
            mode: crate::config::AgentMode::Full,
            read_max_lines: 500,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: Arc::new(crate::config::Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        }
    }

    #[tokio::test]
    async fn ask_null_interface_returns_error() {
        let tool = AskTool;
        let result = tool
            .execute(
                "c1",
                json!({"question": "What color?"}),
                test_ctx(Arc::new(NullInterface)),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        let text = extract_text(&result);
        assert!(text.contains("Cannot access ask_user tool in this mode"));
    }

    #[tokio::test]
    async fn ask_missing_question_returns_error() {
        let tool = AskTool;
        let result = tool
            .execute("c3", json!({}), test_ctx(Arc::new(MockUi::default())))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(extract_text(&result).contains("Missing required parameter: question"));
    }

    #[tokio::test]
    async fn ask_single_choice_returns_structured_answer() {
        let tool = AskTool;
        let ui = MockUi::new().with_select(1);
        let result = tool
            .execute(
                "c4",
                json!({"question": "Pick", "choices": ["Red", "Blue"]}),
                test_ctx(ui),
            )
            .await
            .unwrap();

        assert_eq!(extract_text(&result), "Blue");
        assert_eq!(result.details["answered"], true);
        assert_eq!(result.details["choice_index"], 1);
        assert_eq!(result.details["multi_select"], false);
    }

    #[tokio::test]
    async fn ask_multi_select_returns_structured_answers() {
        let tool = AskTool;
        let ui = MockUi::new().with_multi_select(vec![0, 2]);
        let result = tool
            .execute(
                "c5",
                json!({"question": "Pick", "choices": ["Red", "Blue", "Green"], "multi_select": true}),
                test_ctx(ui),
            )
            .await
            .unwrap();

        assert_eq!(extract_text(&result), "Red, Green");
        assert_eq!(result.details["answers"][0], "Red");
        assert_eq!(result.details["answers"][1], "Green");
        assert_eq!(result.details["multi_select"], true);
    }

    #[tokio::test]
    async fn ask_free_text_uses_placeholder() {
        let tool = AskTool;
        let ui = MockUi::new().with_input("typed");
        let result = tool
            .execute(
                "c6",
                json!({"question": "Name?", "placeholder": "e.g. Atlas"}),
                test_ctx(ui.clone()),
            )
            .await
            .unwrap();

        assert_eq!(extract_text(&result), "typed");
        assert_eq!(
            ui.last_placeholder.lock().unwrap().as_deref(),
            Some("e.g. Atlas")
        );
    }

    #[tokio::test]
    async fn ask_rejects_duplicate_choices() {
        let tool = AskTool;
        let result = tool
            .execute(
                "c7",
                json!({"question": "Pick", "choices": ["Red", "Red"]}),
                test_ctx(Arc::new(MockUi::default())),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(extract_text(&result).contains("duplicate choice"));
    }

    #[derive(Default)]
    struct MockUi {
        select: Mutex<Option<usize>>,
        multi_select: Mutex<Option<Vec<usize>>>,
        input: Mutex<Option<String>>,
        last_placeholder: Mutex<Option<String>>,
    }

    impl MockUi {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn with_select(self: Arc<Self>, value: usize) -> Arc<Self> {
            *self.select.lock().unwrap() = Some(value);
            self
        }
        fn with_multi_select(self: Arc<Self>, value: Vec<usize>) -> Arc<Self> {
            *self.multi_select.lock().unwrap() = Some(value);
            self
        }
        fn with_input(self: Arc<Self>, value: &str) -> Arc<Self> {
            *self.input.lock().unwrap() = Some(value.to_string());
            self
        }
    }

    #[async_trait]
    impl crate::ui::UserInterface for MockUi {
        fn has_ui(&self) -> bool {
            true
        }
        async fn notify(&self, _: &str, _: crate::ui::NotifyLevel) {}
        async fn confirm(&self, _: &str, _: &str) -> Option<bool> {
            None
        }
        async fn select_with_context(&self, _: &str, _: &str, _: &[SelectOption]) -> Option<usize> {
            *self.select.lock().unwrap()
        }
        async fn multi_select_with_context(
            &self,
            _: &str,
            _: &str,
            _: &[SelectOption],
        ) -> Option<Vec<usize>> {
            self.multi_select.lock().unwrap().clone()
        }
        async fn input_with_context(&self, _: &str, _: &str, placeholder: &str) -> Option<String> {
            *self.last_placeholder.lock().unwrap() = Some(placeholder.to_string());
            self.input.lock().unwrap().clone()
        }
        async fn set_status(&self, _: &str, _: Option<&str>) {}
        async fn set_widget(&self, _: &str, _: Option<crate::ui::WidgetContent>) {}
        async fn custom(&self, _: crate::ui::ComponentSpec) -> Option<serde_json::Value> {
            None
        }
    }

    fn extract_text(output: &ToolOutput) -> String {
        output
            .content
            .iter()
            .filter_map(|b| match b {
                imp_llm::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
