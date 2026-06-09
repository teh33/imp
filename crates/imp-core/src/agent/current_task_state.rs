use imp_llm::{ContentBlock, Message};

use super::{Agent, TurnState};
use crate::workflow::{VerificationGate, WorkflowContract};

const MAX_OBJECTIVE_CHARS: usize = 700;
const MAX_MESSAGE_CHARS: usize = 500;
const MAX_VERIFICATION_GATES: usize = 8;
const MAX_RECENT_USER_MESSAGES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CurrentTaskState {
    objective: Option<String>,
    workflow: Vec<String>,
    turn: Vec<String>,
    verification: Vec<String>,
    recent_user_constraints: Vec<String>,
}

impl CurrentTaskState {
    fn from_agent(agent: &Agent, turn_state: &TurnState) -> Self {
        let mut state = Self::default();

        state.objective = agent
            .active_objective
            .as_ref()
            .map(|objective| truncate_for_state(&objective.prompt, MAX_OBJECTIVE_CHARS))
            .filter(|objective| !objective.trim().is_empty());

        state.workflow = workflow_lines(agent.workflow_contract());
        state.turn = turn_lines(turn_state);
        state.verification = verification_lines(&agent.verification_gates);
        state.recent_user_constraints = recent_user_lines(&agent.messages);

        state
    }

    fn is_empty(&self) -> bool {
        self.objective.is_none()
            && self.workflow.is_empty()
            && self.turn.is_empty()
            && self.verification.is_empty()
            && self.recent_user_constraints.is_empty()
    }

    fn render(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        let mut out = String::from("# Current Task State\n");
        out.push_str("This block is runtime state. Prefer it over stale chat history when deciding what to do next.\n");

        if let Some(objective) = &self.objective {
            out.push_str("\nObjective:\n");
            out.push_str("- ");
            out.push_str(objective);
            out.push('\n');
        }

        push_section(&mut out, "Workflow", &self.workflow);
        push_section(&mut out, "Turn", &self.turn);
        push_section(&mut out, "Verification", &self.verification);
        push_section(
            &mut out,
            "Recent User Messages",
            &self.recent_user_constraints,
        );

        Some(out)
    }
}

impl Agent {
    pub(in crate::agent) fn system_prompt_with_current_task_state(
        &self,
        turn_state: &TurnState,
    ) -> String {
        let Some(state) = CurrentTaskState::from_agent(self, turn_state).render() else {
            return self.system_prompt.clone();
        };
        if self.system_prompt.trim().is_empty() {
            state
        } else {
            format!("{}\n\n{}", self.system_prompt, state)
        }
    }
}

fn workflow_lines(contract: &WorkflowContract) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(id) = contract
        .id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("id: {id}"));
    }
    if let Some(unit) = contract
        .workflow_unit_ref
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("unit: {unit}"));
    }
    if let Some(title) = contract
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("title: {}", truncate_for_state(title, 160)));
    }
    if !contract.objective.trim().is_empty() {
        lines.push(format!(
            "contract objective: {}",
            truncate_for_state(&contract.objective, MAX_OBJECTIVE_CHARS)
        ));
    }
    lines.push(format!("type: {:?}", contract.workflow_type));
    lines.push(format!("risk: {:?}", contract.risk_level));
    lines.push(format!("autonomy: {}", contract.autonomy_mode));
    if let Some(role) = contract
        .role
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("role: {role}"));
    }
    lines
}

fn turn_lines(turn_state: &TurnState) -> Vec<String> {
    let mut lines = vec![
        format!("index: {}", turn_state.index),
        format!("phase: {}", turn_state.phase.as_str()),
    ];
    if let Some(reason) = turn_state.continue_reason {
        lines.push(format!("continue reason: {}", reason.as_str()));
    }
    if turn_state.planned_tools > 0 || turn_state.completed_tools > 0 {
        lines.push(format!(
            "tools: {}/{} completed",
            turn_state.completed_tools, turn_state.planned_tools
        ));
    }
    lines
}

fn verification_lines(gates: &[VerificationGate]) -> Vec<String> {
    let mut lines = gates
        .iter()
        .take(MAX_VERIFICATION_GATES)
        .map(|gate| {
            let command = gate
                .command
                .as_ref()
                .map(|command| format!(": {}", truncate_for_state(&command.command, 220)))
                .unwrap_or_default();
            format!(
                "{} [{:?}/{:?}]{}",
                gate.name, gate.requirement, gate.status, command
            )
        })
        .collect::<Vec<_>>();
    if gates.len() > MAX_VERIFICATION_GATES {
        lines.push(format!(
            "{} additional verification gate(s) omitted",
            gates.len() - MAX_VERIFICATION_GATES
        ));
    }
    lines
}

fn recent_user_lines(messages: &[Message]) -> Vec<String> {
    let mut lines = messages
        .iter()
        .rev()
        .filter_map(|message| match message {
            Message::User(user) => {
                let text = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = text.trim();
                (!text.is_empty()).then(|| truncate_for_state(text, MAX_MESSAGE_CHARS))
            }
            _ => None,
        })
        .take(MAX_RECENT_USER_MESSAGES)
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

fn push_section(out: &mut String, title: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str(title);
    out.push_str(":\n");
    for line in lines {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
}

fn truncate_for_state(text: &str, max_chars: usize) -> String {
    let mut normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    normalized.push('…');
    normalized
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::model::{Capabilities, ModelPricing};
    use imp_llm::provider::{Provider, RequestOptions};
    use imp_llm::{Model, ModelMeta, StreamEvent};

    use super::*;
    use crate::agent::TurnState;
    use crate::workflow::{VerificationGate, WorkflowContract};

    struct NullProvider;

    #[async_trait]
    impl Provider for NullProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: imp_llm::Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            Box::pin(futures::stream::empty())
        }

        async fn resolve_auth(
            &self,
            _auth: &imp_llm::auth::AuthStore,
        ) -> imp_llm::Result<imp_llm::auth::ApiKey> {
            Ok("test".into())
        }

        fn id(&self) -> &str {
            "null"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    fn test_agent() -> Agent {
        let model = Model {
            meta: ModelMeta {
                id: "test".into(),
                provider: "test".into(),
                name: "Test".into(),
                context_window: 100_000,
                max_output_tokens: 4096,
                pricing: ModelPricing::default(),
                capabilities: Capabilities::default(),
            },
            provider: Arc::new(NullProvider),
        };
        let (mut agent, _handle) = Agent::new(model, PathBuf::from("/tmp/project"));
        agent.system_prompt = "base prompt".to_string();
        agent.active_objective = Some(crate::agent::autonomy::AutonomousObjective::from_prompt(
            "Fix the context compaction regression without losing user instructions.",
        ));
        agent.messages.push(Message::user("Keep this scoped."));
        agent.set_workflow_contract(
            WorkflowContract::implicit("Fix the context compaction regression")
                .with_workflow_unit_ref("context-gc"),
        );
        agent.verification_gates = vec![VerificationGate::command(
            "focused-test",
            "cargo test -p imp-core current_task_state",
        )];
        agent
    }

    #[test]
    fn current_task_state_appends_compact_runtime_state() {
        let agent = test_agent();
        let mut turn = TurnState::new(2);
        turn.enter(crate::agent::TurnPhase::BuildContext);

        let prompt = agent.system_prompt_with_current_task_state(&turn);

        assert!(prompt.starts_with("base prompt\n\n# Current Task State"));
        assert!(prompt.contains("Objective:"));
        assert!(prompt.contains("unit: context-gc"));
        assert!(prompt.contains("phase: build_context"));
        assert!(prompt.contains("focused-test"));
        assert!(prompt.contains("Keep this scoped."));
    }
}
