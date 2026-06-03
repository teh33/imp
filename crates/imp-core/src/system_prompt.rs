use std::fmt;

use crate::config::AgentMode;
use crate::context::estimate_tokens;
use crate::guardrails::{self, GuardrailProfile};
use crate::resources::{AgentsMd, Skill, SoulDoc};
use crate::roles::Role;
use crate::soul::{soul_identity_text, DEFAULT_IDENTITY};
use crate::tools::ToolRegistry;

/// A project fact from durable project context.
#[derive(Debug, Clone)]
pub struct Fact {
    pub text: String,
    pub verified_ago: String,
}

/// Previous attempt info for task context.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub number: u32,
    pub outcome: String,
    pub summary: String,
}

/// Dependency info for task context.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub status: String,
    pub detail: String,
}

/// Task context for headless/task mode (Layer 5).
#[derive(Debug, Clone)]
pub struct TaskContext {
    pub title: String,
    pub description: String,
    pub design: Option<String>,
    pub acceptance: Option<String>,
    pub verify: Option<String>,
    pub verify_timeout_secs: Option<u64>,
    pub fail_first: bool,
    pub notes: Option<String>,
    pub attempts: Vec<Attempt>,
    pub dependencies: Vec<Dependency>,
    pub decisions: Vec<String>,
    pub context_paths: Vec<String>,
    pub constraints: Vec<String>,
}

/// Result of system prompt assembly, including size tracking.
#[derive(Debug)]
pub struct AssembledPrompt {
    pub text: String,
    pub estimated_tokens: u32,
}

impl fmt::Display for AssembledPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// All inputs needed to assemble a system prompt.
pub struct AssembleParams<'a> {
    pub tools: &'a ToolRegistry,
    pub agents_md: &'a [AgentsMd],
    pub skills: &'a [Skill],
    pub facts: &'a [Fact],
    pub project_memory_status: Option<&'a str>,
    pub soul: Option<&'a SoulDoc>,
    pub task: Option<&'a TaskContext>,
    pub role: Option<&'a Role>,
    pub mode: &'a AgentMode,
    pub memory: Option<&'a str>,
    pub user_profile: Option<&'a str>,
    pub cwd: Option<&'a std::path::Path>,
    pub repo_context: Option<&'a crate::repo_intelligence::RepoContextSummary>,
    /// Whether to include learning instructions in the system prompt.
    pub learning_enabled: bool,
    /// Resolved guardrail profile (None = guardrails disabled).
    pub guardrail_profile: Option<GuardrailProfile>,
}

/// Assemble the system prompt from seven layers.
///
/// - Layer 1: Identity + tool descriptions (+ role instructions if any)
/// - Layer 1.25: Execution policy
/// - Layer 1.5: Environment context
/// - Layer 2: Project context from AGENTS.md files
/// - Layer 3: Skills index
/// - Layer 4: Workflow facts (skipped if empty)
/// - Layer 4.25: Compact project memory status (skipped if empty)
/// - Layer 5: Task context (only in headless/task mode)
/// - Layer 6: Agent memory (if present)
pub fn assemble(params: &AssembleParams<'_>) -> AssembledPrompt {
    assemble_inner(params)
}

fn assemble_inner(p: &AssembleParams<'_>) -> AssembledPrompt {
    let mut parts = Vec::new();

    // Layer 1: Identity + tool descriptions
    parts.push(identity_layer(
        p.tools,
        p.role,
        p.mode,
        p.learning_enabled,
        p.soul,
    ));

    // Layer 1.25: Execution policy (currently folded into identity operating rules)
    let execution_policy = execution_policy_layer();
    if !execution_policy.is_empty() {
        parts.push(execution_policy);
    }

    // Layer 1.5: Environment context
    parts.push(environment_layer(p.cwd));

    // Layer 1.75: Repo intelligence context
    if let Some(repo_context) = p.repo_context {
        parts.push(repo_context.render_prompt_layer());
    }

    // Layer 2: Project context from AGENTS.md
    if !p.agents_md.is_empty() {
        parts.push(agents_md_layer(p.agents_md));
    }

    // Layer 3: Skills index
    if !p.skills.is_empty() {
        parts.push(skills_layer(p.skills, p.mode));
    }

    // Layer 4: Workflow facts
    if !p.facts.is_empty() {
        parts.push(facts_layer(p.facts));
    }

    // Layer 4.25: Compact project memory status
    if let Some(status) = p.project_memory_status {
        if !status.is_empty() {
            parts.push(project_memory_status_layer(status));
        }
    }

    // Layer 4.5: Engineering guardrails (when enabled)
    if let Some(profile) = p.guardrail_profile {
        parts.push(guardrails::guardrails_layer(profile));
    }

    // Layer 5: Task context (headless mode only)
    if let Some(task) = p.task {
        parts.push(task_layer(task));
        parts.push(headless_execution_layer(task));
    }

    // Layer 6: Agent memory
    if let Some(mem) = p.memory {
        if !mem.is_empty() {
            parts.push(mem.to_string());
        }
    }
    if let Some(user) = p.user_profile {
        if !user.is_empty() {
            parts.push(user.to_string());
        }
    }

    let text = parts.join("\n\n");
    let estimated_tokens = estimate_tokens(&text);

    AssembledPrompt {
        text,
        estimated_tokens,
    }
}

fn identity_layer(
    tools: &ToolRegistry,
    role: Option<&Role>,
    mode: &AgentMode,
    learning_enabled: bool,
    soul: Option<&SoulDoc>,
) -> String {
    let mut s = String::new();
    if let Some(soul) = soul {
        s.push_str(&soul_identity_text(&soul.content));
    } else {
        s.push_str(DEFAULT_IDENTITY);
    }
    s.push_str("\n\nAvailable tools:\n");

    let defs = match role {
        Some(r) if r.readonly => tools.readonly_definitions(),
        _ => tools.definitions_for_mode(mode),
    };

    for def in &defs {
        s.push_str(&format!("- {}: {}\n", def.name, def.description));
    }

    if let Some(soul) = soul {
        s.push_str("\n\nSoul:\n");
        s.push_str(&soul.content);
        s.push('\n');
    }

    s.push_str("\nTool routing:\n");
    s.push_str("- For code lookup, use `scan` for structure and `rg` for raw text; avoid unpruned `find`. Use `bash` for builds, tests, scripts, and package managers.\n");
    if defs.iter().any(|def| def.name == "git") {
        s.push_str("- Use `git` for local repo/worktree operations; use `bash` for uncovered git commands.\n");
    }
    if defs.iter().any(|def| def.name == "workflow") {
        s.push_str("- Use `workflow` for durable project plans, schema-checked status updates, validation, and orchestrated workflow steps.\n");
    }
    s.push_str("- Use `read` before explaining or editing specific files; use `edit`/`write` for file changes.\n");

    // Append role instructions after identity layer
    if let Some(role) = role {
        if let Some(ref instructions) = role.instructions {
            s.push('\n');
            s.push_str(instructions);
            s.push('\n');
        }
        for instruction in &role.instruction_set {
            if role.instructions.as_ref() == Some(instruction) {
                continue;
            }
            s.push('\n');
            s.push_str(instruction);
            s.push('\n');
        }
        if let Some(schema) = &role.output_schema {
            s.push_str("\nRole output schema metadata:\n");
            if !schema.name.is_empty() {
                s.push_str("- schema: `");
                s.push_str(&schema.name);
                s.push_str("`\n");
            }
            if !schema.description.is_empty() {
                s.push_str("- description: ");
                s.push_str(&schema.description);
                s.push('\n');
            }
            if let Some(reference) = &schema.json_schema_ref {
                s.push_str("- json_schema_ref: ");
                s.push_str(reference);
                s.push('\n');
            }
            if !schema.required_sections.is_empty() {
                s.push_str("- required sections: ");
                s.push_str(&schema.required_sections.join(", "));
                s.push('\n');
            }
            if let Some(contract) = &schema.output_contract {
                s.push_str("- output contract: ");
                s.push_str(contract);
                s.push('\n');
            }
            if let Some(example) = &schema.example {
                s.push_str("- example:\n");
                s.push_str(example);
                s.push('\n');
            }
            s.push_str("These schema hints guide the response shape; they are metadata, not enforced validation.\n");
        }
    }

    // Append mode instructions if present
    if let Some(instructions) = mode.instructions() {
        s.push('\n');
        s.push_str(instructions);
        s.push('\n');
    }

    // Append learning instructions when enabled
    if learning_enabled {
        s.push('\n');
        s.push_str(crate::learning::LEARNING_INSTRUCTIONS);
        s.push('\n');
    }

    s
}

fn execution_policy_layer() -> String {
    String::new()
}

fn environment_layer(cwd: Option<&std::path::Path>) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd_str = cwd.map(|p| p.display().to_string()).unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });
    let os = std::env::consts::OS;
    let today = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days = secs / 86400;
        // Simple date calculation
        let (y, m, d) = days_to_ymd(days);
        format!("{y}-{m:02}-{d:02}")
    };
    format!("Environment: cwd={cwd_str}, os={os}, home={home}, date={today}")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Civil days algorithm (Howard Hinnant)
    days += 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn agents_md_layer(agents: &[AgentsMd]) -> String {
    let mut s = String::from("# Project Context\n\n");
    for agent in agents {
        s.push_str(&agent.content);
        s.push('\n');
    }
    s
}

fn skills_layer(skills: &[Skill], _mode: &AgentMode) -> String {
    let mut s = String::from(
        "Available skills (load with `read ~/.imp/skills/<name>/SKILL.md` when relevant):\n",
    );
    for skill in skills {
        let description = compact_skill_description(&skill.description);
        if description.is_empty() {
            s.push_str(&format!("- {}\n", skill.name));
        } else {
            s.push_str(&format!("- {}: {}\n", skill.name, description));
        }
    }
    s
}

fn compact_skill_description(description: &str) -> String {
    let normalized = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let first_sentence = normalized
        .split_once(". ")
        .map(|(first, _)| format!("{}.", first))
        .unwrap_or(normalized);
    truncate_chars(&first_sentence, 120)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn facts_layer(facts: &[Fact]) -> String {
    let mut s = String::from("Project facts:\n");
    for fact in facts {
        s.push_str(&format!(
            "- \"{}\" [verified {}]\n",
            fact.text, fact.verified_ago
        ));
    }
    s
}

fn project_memory_status_layer(status: &str) -> String {
    status.to_string()
}

fn task_layer(task: &TaskContext) -> String {
    let mut s = String::from("## Task\n");
    s.push_str(&format!("Title: {}\n", task.title));
    s.push_str(&format!("Description: {}\n", task.description));
    if let Some(ref design) = task.design {
        if !design.trim().is_empty() {
            s.push_str("Design:\n");
            s.push_str(design);
            s.push('\n');
        }
    }
    if let Some(ref notes) = task.notes {
        if !notes.trim().is_empty() {
            s.push_str("Notes:\n");
            s.push_str(notes);
            s.push('\n');
        }
    }
    if let Some(ref acceptance) = task.acceptance {
        s.push_str("Acceptance:\n");
        s.push_str(acceptance);
        s.push('\n');
    }
    if let Some(ref verify) = task.verify {
        s.push_str(&format!("Verify: {}\n", verify));
        if let Some(timeout_secs) = task.verify_timeout_secs {
            s.push_str(&format!("Verify timeout: {}s\n", timeout_secs));
        }
        if task.fail_first {
            s.push_str("Fail-first: verify was expected to fail before implementation; preserve that contract.\n");
        }
        s.push_str("Treat the verify command as the primary completion check for this task.\n");
    }

    if !task.context_paths.is_empty() {
        s.push_str("\n## Referenced files\n");
        s.push_str("Use these declared file/path hints before broadening the search.\n");
        for path in &task.context_paths {
            s.push_str(&format!("- {}\n", path));
        }
    }

    if !task.constraints.is_empty() {
        s.push_str("\n## Constraints\n");
        for constraint in &task.constraints {
            s.push_str(&format!("- {}\n", constraint));
        }
    }

    if !task.attempts.is_empty() {
        s.push_str("\n## Previous attempts\n");
        s.push_str("Do not repeat a failed approach unchanged; use the attempt history to adjust your plan.\n");
        for attempt in &task.attempts {
            s.push_str(&format!(
                "Attempt {} ({}): {}\n",
                attempt.number, attempt.outcome, attempt.summary
            ));
        }
    }

    if !task.dependencies.is_empty() {
        s.push_str("\n## Dependencies\n");
        s.push_str("Respect dependency state when sequencing work; unresolved dependencies are potential blockers.\n");
        for dep in &task.dependencies {
            s.push_str(&format!(
                "- {} ({}): {}\n",
                dep.name, dep.status, dep.detail
            ));
        }
    }

    if !task.decisions.is_empty() {
        s.push_str("\n## Unresolved decisions\n");
        s.push_str("These decisions block fully autonomous execution; resolve them or surface them clearly instead of guessing.\n");
        for decision in &task.decisions {
            s.push_str(&format!("- {}\n", decision));
        }
    }

    s
}

fn headless_execution_layer(task: &TaskContext) -> String {
    let mut s = String::from("## Headless execution contract\n");
    s.push_str("- You are executing an explicit workflow task, not exploring broadly.\n");
    s.push_str("- Treat the task title, description, notes, acceptance criteria, and verify gate as the source of truth for scope and success.\n");
    s.push_str("- Execute the assigned outcome before expanding into adjacent cleanup, refactors, or unrelated improvements.\n");
    s.push_str("- Use explicit file references and prefilled context first before searching more broadly.\n");
    s.push_str(
        "- If the task includes prior failed attempts, do not retry the same plan unchanged.\n",
    );
    s.push_str("- If dependency state or prerequisite decisions are unresolved, treat that as a blocker rather than improvising around it.\n");
    s.push_str("- Keep progress updates concise and useful. Record meaningful discoveries, blockers, and revised plans with native workflow updates.\n");
    if task.verify.is_some() {
        s.push_str("- If the verify command fails, either fix the issue or report the exact blocker. Do not claim completion anyway.\n");
    }
    s.push_str("- In batch-verify flows, treat your goal as leaving the task ready for verify rather than assuming verify already passed.\n");
    s.push_str(
        "- Respect parent/child structure: finish this task's outcome, not the whole feature.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::tools::{Tool, ToolContext, ToolOutput};
    use async_trait::async_trait;

    // -- Test tool helpers --

    struct FakeTool {
        name: &'static str,
        description: &'static str,
        readonly: bool,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn label(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            self.description
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_readonly(&self) -> bool {
            self.readonly
        }
        async fn execute(
            &self,
            _: &str,
            _: serde_json::Value,
            _: ToolContext,
        ) -> crate::Result<ToolOutput> {
            Ok(ToolOutput::text("ok"))
        }
    }

    fn make_registry() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(FakeTool {
            name: "read",
            description: "Read file contents",
            readonly: true,
        }));
        reg.register(Arc::new(FakeTool {
            name: "write",
            description: "Write content to a file",
            readonly: false,
        }));
        reg.register(Arc::new(FakeTool {
            name: "edit",
            description: "Edit a file by replacing exact text",
            readonly: false,
        }));
        reg.register(Arc::new(FakeTool {
            name: "bash",
            description: "Run shell commands",
            readonly: false,
        }));
        reg
    }

    fn make_skill(name: &str, desc: &str, path: &str) -> Skill {
        Skill {
            name: name.into(),
            description: desc.into(),
            path: PathBuf::from(path),
        }
    }

    fn make_agents_md(content: &str) -> AgentsMd {
        AgentsMd {
            path: PathBuf::from("/project/AGENTS.md"),
            content: content.into(),
        }
    }

    fn make_readonly_role() -> Role {
        Role::from_def(
            "reviewer",
            &crate::roles::RoleDef {
                readonly: true,
                instructions: Some("Review code carefully. Do not modify files.".into()),
                ..crate::roles::RoleDef::default()
            },
        )
    }

    fn make_worker_role() -> Role {
        Role::from_def("worker", &crate::roles::RoleDef::default())
    }

    /// Test helper: shorthand for assemble() with no memory/user_profile.
    fn test_assemble(
        tools: &ToolRegistry,
        agents_md: &[AgentsMd],
        skills: &[Skill],
        facts: &[Fact],
        task: Option<&TaskContext>,
        role: Option<&Role>,
    ) -> AssembledPrompt {
        assemble(&AssembleParams {
            tools,
            agents_md,
            skills,
            facts,
            project_memory_status: None,
            soul: None,
            task,
            role,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        })
    }

    // -- Layer 1: Identity --

    #[test]
    fn system_prompt_omits_generated_operating_rules() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("Operating rules:"));
        assert!(!result.text.contains(
            "Ground repository claims in files or tool output inspected in this session"
        ));
        assert!(!result.text.contains(
            "For analysis-only requests, stay read-only. For implementation, make small reversible changes"
        ));
    }

    #[test]
    fn system_prompt_omits_clarification_and_natural_closeout_guidance() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains(
            "Ask a focused clarification before continuing when the user asks to continue a plan"
        ));
        assert!(!result
            .text
            .contains("internal closeout semantics, not mandatory response headings"));
        assert!(!result
            .text
            .contains("Use natural conversational replies unless the user explicitly asks"));
    }

    #[test]
    fn system_prompt_omits_conversation_time_workflow_planning_doctrine() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("For durable project work"));
        assert!(!result.text.contains("tied to an adopted goal"));
        assert!(!result
            .text
            .contains("workflow steps/checks/acceptance/decisions"));
        assert!(!result.text.contains(
            "Record progress after failures or material planning changes before relying on chat memory"
        ));
        assert!(!result
            .text
            .contains("workflow updates are checkpoints, not proof of completion"));
        assert!(!result.text.contains("explanation-only answers"));
        assert!(!result.text.contains("Imp-work doctrine:"));
        assert!(!result
            .text
            .contains("between-turn workflow update before the substantive reply"));
        assert!(!result
            .text
            .contains("include a concise workflow delta summary in the response"));
    }

    #[test]
    fn system_prompt_identity_includes_all_tools() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(result
            .text
            .contains("You are imp, a professional coding agent."));
        assert!(result.text.contains("- read: Read file contents"));
        assert!(result.text.contains("- write: Write content to a file"));
        assert!(result
            .text
            .contains("- edit: Edit a file by replacing exact text"));
        assert!(result.text.contains("- bash: Run shell commands"));
    }

    #[test]
    fn system_prompt_workflow_guidance_prefers_native_tool_when_available() {
        let mut reg = make_registry();
        reg.register(Arc::new(FakeTool {
            name: "workflow",
            description: "Workflowge imp-native workflows",
            readonly: false,
        }));

        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(result
            .text
            .contains("Use `workflow` for durable project plans"));
        assert!(!result
            .text
            .contains("Use native workflows when durable work"));
        assert!(!result.text.contains("Use workflow when durable work"));
    }

    #[test]
    fn system_prompt_workflow_guidance_omitted_without_workflow_tool() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result
            .text
            .contains("Use `workflow` for durable project plans"));
    }

    #[test]
    fn system_prompt_no_legacy_workflow_guidance_or_delegation_in_prompt() {
        // Extended workflow guidance lives in native workflow affordances.
        // Verify large legacy prompt blocks no longer appear regardless of tool availability.
        let mut reg = make_registry();
        reg.register(Arc::new(FakeTool {
            name: "bash",
            description: "Run shell commands",
            readonly: false,
        }));
        reg.register(Arc::new(FakeTool {
            name: "workflow",
            description: "Workflowge workflow work",
            readonly: false,
        }));

        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(
            !result.text.contains("Workflow guidance:"),
            "workflow guidance block should not appear in system prompt"
        );
        assert!(
            !result.text.contains("## Workflow delegation"),
            "delegation guidance should not appear in system prompt"
        );
    }

    #[test]
    fn system_prompt_identity_only_when_all_layers_empty() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        // Should have identity but no section headers for missing layers
        assert!(result.text.contains("You are imp"));
        assert!(!result.text.contains("# Project Context"));
        assert!(!result.text.contains("Available skills"));
        assert!(!result.text.contains("Project facts"));
        assert!(!result.text.contains("## Task"));
    }

    #[test]
    fn system_prompt_agents_md_included_verbatim() {
        let reg = make_registry();
        let agents = vec![make_agents_md("# Rules\n\nUse snake_case everywhere.")];
        let result = test_assemble(&reg, &agents, &[], &[], None, None);
        assert!(result.text.contains("# Project Context"));
        assert!(result
            .text
            .contains("# Rules\n\nUse snake_case everywhere."));
    }

    #[test]
    fn system_prompt_multiple_agents_md_concatenated() {
        let reg = make_registry();
        let agents = vec![
            make_agents_md("Global rules here."),
            make_agents_md("Project rules here."),
        ];
        let result = test_assemble(&reg, &agents, &[], &[], None, None);
        assert!(result.text.contains("Global rules here."));
        assert!(result.text.contains("Project rules here."));
    }

    #[test]
    fn system_prompt_empty_agents_md_skipped() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("# Project Context"));
    }

    // -- Layer 3: Skills --

    #[test]
    fn system_prompt_skills_listed_compactly_without_paths() {
        let reg = make_registry();
        let skills = vec![
            make_skill(
                "rust",
                "Conventions for Rust code. Extra detail that should not be included.",
                "/home/.imp/skills/rust/SKILL.md",
            ),
            make_skill(
                "testing",
                "Write and review tests",
                "/home/.imp/skills/testing/SKILL.md",
            ),
        ];
        let result = test_assemble(&reg, &[], &skills, &[], None, None);
        assert!(result.text.contains(
            "Available skills (load with `read ~/.imp/skills/<name>/SKILL.md` when relevant):"
        ));
        assert!(result.text.contains("- rust: Conventions for Rust code."));
        assert!(result.text.contains("- testing: Write and review tests"));
        assert!(!result.text.contains("/home/.imp/skills/rust/SKILL.md"));
        assert!(!result
            .text
            .contains("Extra detail that should not be included"));
    }

    #[test]
    fn system_prompt_does_not_add_mode_aware_workflow_skill_trigger() {
        let reg = make_registry();
        let skills = vec![make_skill(
            "workflow",
            "Coordinate explicit work through workflows",
            "/home/.imp/skills/workflow/SKILL.md",
        )];
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &skills,
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Planner,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        assert!(!result.text.contains("- Trigger:"));
        assert!(!result.text.contains("Load `workflow`"));
    }

    #[test]
    fn system_prompt_orchestrator_does_not_add_workflow_skill_trigger() {
        let reg = make_registry();
        let skills = vec![make_skill(
            "workflow",
            "Coordinate explicit work through workflows",
            "/home/.imp/skills/workflow/SKILL.md",
        )];
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &skills,
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Orchestrator,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        assert!(!result.text.contains("- Trigger:"));
        assert!(!result.text.contains("Load `workflow`"));
    }

    #[test]
    fn system_prompt_worker_does_not_add_workflow_basics_trigger() {
        let reg = make_registry();
        let skills = vec![
            make_skill(
                "workflow",
                "Coordinate multi-step work through workflows",
                "/home/.imp/skills/workflow/SKILL.md",
            ),
            make_skill(
                "workflow-basics",
                "Use native workflow actions safely and efficiently",
                "/home/.imp/skills/workflow-basics/SKILL.md",
            ),
        ];
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &skills,
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Worker,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        assert!(!result.text.contains("- Trigger:"));
        assert!(!result.text.contains("Load `workflow-basics`"));
    }

    #[test]
    fn system_prompt_omits_workflow_trigger_without_workflow_skill() {
        let reg = make_registry();
        let skills = vec![make_skill(
            "rust",
            "Conventions for Rust code",
            "/home/.imp/skills/rust/SKILL.md",
        )];
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &skills,
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Planner,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        assert!(!result.text.contains("- Trigger:"));
    }

    #[test]
    fn system_prompt_reviewer_mode_omits_workflow_trigger() {
        let reg = make_registry();
        let skills = vec![make_skill(
            "workflow",
            "Coordinate multi-step work through workflows",
            "/home/.imp/skills/workflow/SKILL.md",
        )];
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &skills,
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Reviewer,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        assert!(!result.text.contains("- Trigger:"));
    }

    #[test]
    fn system_prompt_empty_skills_skipped() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("Available skills"));
    }

    // -- Layer 4: Workflow facts --

    #[test]
    fn system_prompt_facts_included() {
        let reg = make_registry();
        let facts = vec![
            Fact {
                text: "Uses JWT for auth".into(),
                verified_ago: "2h ago".into(),
            },
            Fact {
                text: "Test suite requires Docker".into(),
                verified_ago: "1d ago".into(),
            },
        ];
        let result = test_assemble(&reg, &[], &[], &facts, None, None);
        assert!(result.text.contains("Project facts:"));
        assert!(result
            .text
            .contains("\"Uses JWT for auth\" [verified 2h ago]"));
        assert!(result
            .text
            .contains("\"Test suite requires Docker\" [verified 1d ago]"));
    }

    #[test]
    fn system_prompt_empty_facts_skipped() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("Project facts"));
    }

    #[test]
    fn system_prompt_project_memory_status_included() {
        let reg = make_registry();
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: Some(
                "Project memory status:\nWarnings:\n- STALE: \"Lockfile drift\"\n\nWorking on:\n- [12] Refresh auth flow",
            ),
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(result.text.contains("Project memory status:"));
        assert!(result.text.contains("Warnings:"));
        assert!(result.text.contains("Working on:"));
    }

    #[test]
    fn system_prompt_project_memory_status_empty_string_is_skipped() {
        let reg = make_registry();
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: Some(""),
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(!result.text.contains("Project memory status:"));
    }

    #[test]
    fn system_prompt_project_memory_status_included_separately_from_facts() {
        let reg = make_registry();
        let facts = vec![Fact {
            text: "Uses JWT for auth".into(),
            verified_ago: "2h ago".into(),
        }];
        let status =
            "Project memory status:\nWarnings:\n- stale fact\n\nWorking on:\n- [7] Fix auth flow";
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &facts,
            project_memory_status: Some(status),
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        let facts_pos = result.text.find("Project facts:").unwrap();
        let status_pos = result.text.find("Project memory status:").unwrap();
        assert!(result
            .text
            .contains("\"Uses JWT for auth\" [verified 2h ago]"));
        assert!(result.text.contains("Warnings:"));
        assert!(result.text.contains("Working on:"));
        assert!(facts_pos < status_pos);
    }

    // -- Layer 5: Task context --

    #[test]
    fn system_prompt_task_context_included() {
        let reg = make_registry();
        let task = TaskContext {
            title: "Fix the failing auth test".into(),
            description: "The JWT validation test panics on expired tokens".into(),
            design: None,
            acceptance: None,
            verify: Some("cargo test auth::jwt_test".into()),
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };
        let result = test_assemble(&reg, &[], &[], &[], Some(&task), None);
        assert!(result.text.contains("## Task"));
        assert!(result.text.contains("Title: Fix the failing auth test"));
        assert!(result
            .text
            .contains("Description: The JWT validation test panics"));
        assert!(result.text.contains("Verify: cargo test auth::jwt_test"));
        assert!(result
            .text
            .contains("Treat the verify command as the primary completion check for this task."));
    }

    #[test]
    fn system_prompt_task_with_attempts() {
        let reg = make_registry();
        let task = TaskContext {
            title: "Fix bug".into(),
            description: "Something is broken".into(),
            design: None,
            acceptance: None,
            verify: None,
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![
                Attempt {
                    number: 1,
                    outcome: "failed".into(),
                    summary: "Tried X, got error Y".into(),
                },
                Attempt {
                    number: 2,
                    outcome: "failed".into(),
                    summary: "Tried Z, still broken".into(),
                },
            ],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };
        let result = test_assemble(&reg, &[], &[], &[], Some(&task), None);
        assert!(result.text.contains("## Previous attempts"));
        assert!(result.text.contains(
            "Do not repeat a failed approach unchanged; use the attempt history to adjust your plan."
        ));
        assert!(result
            .text
            .contains("Attempt 1 (failed): Tried X, got error Y"));
        assert!(result
            .text
            .contains("Attempt 2 (failed): Tried Z, still broken"));
    }

    #[test]
    fn system_prompt_task_with_dependencies() {
        let reg = make_registry();
        let task = TaskContext {
            title: "Implement feature".into(),
            description: "New feature".into(),
            design: None,
            acceptance: None,
            verify: None,
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![],
            dependencies: vec![Dependency {
                name: "Schema types".into(),
                status: "completed".into(),
                detail: "defined in src/schema.rs".into(),
            }],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };
        let result = test_assemble(&reg, &[], &[], &[], Some(&task), None);
        assert!(result.text.contains("## Dependencies"));
        assert!(result.text.contains(
            "Respect dependency state when sequencing work; unresolved dependencies are potential blockers."
        ));
        assert!(result
            .text
            .contains("- Schema types (completed): defined in src/schema.rs"));
    }

    #[test]
    fn system_prompt_task_with_notes_and_context_paths() {
        let reg = make_registry();
        let task = TaskContext {
            title: "Fix auth".into(),
            description: "Tighten token validation".into(),
            design: Some(
                "Keep validation logic in the existing auth module; avoid a broader auth rewrite."
                    .into(),
            ),
            acceptance: None,
            verify: Some("cargo test auth".into()),
            verify_timeout_secs: Some(30),
            fail_first: true,
            notes: Some("Prefer touching only auth paths unless necessary".into()),
            attempts: vec![],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec!["src/auth.rs".into(), "tests/auth.rs".into()],
            constraints: vec![
                "Scope changes to auth-related files unless broader edits are necessary".into(),
            ],
        };
        let result = test_assemble(&reg, &[], &[], &[], Some(&task), None);
        assert!(result.text.contains("Design:"));
        assert!(result
            .text
            .contains("Keep validation logic in the existing auth module"));
        assert!(result.text.contains("Verify timeout: 30s"));
        assert!(result
            .text
            .contains("Fail-first: verify was expected to fail before implementation"));
        assert!(result.text.contains("Notes:"));
        assert!(result
            .text
            .contains("Prefer touching only auth paths unless necessary"));
        assert!(result.text.contains("## Referenced files"));
        assert!(result.text.contains("- src/auth.rs"));
        assert!(result.text.contains("- tests/auth.rs"));
        assert!(result.text.contains("## Constraints"));
        assert!(result
            .text
            .contains("Scope changes to auth-related files unless broader edits are necessary"));
    }

    #[test]
    fn system_prompt_no_task_skips_layer5() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(!result.text.contains("## Task"));
    }

    #[test]
    fn system_prompt_task_without_verify_omits_verify_line() {
        let reg = make_registry();
        let task = TaskContext {
            title: "Do something".into(),
            description: "Details here".into(),
            design: None,
            acceptance: None,
            verify: None,
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };
        let result = test_assemble(&reg, &[], &[], &[], Some(&task), None);
        assert!(result.text.contains("Title: Do something"));
        assert!(!result.text.contains("Verify:"));
    }

    // -- Role-aware assembly --

    #[test]
    fn system_prompt_readonly_role_filters_tools() {
        let reg = make_registry();
        let role = make_readonly_role();
        let result = test_assemble(&reg, &[], &[], &[], None, Some(&role));
        // Should include readonly tools
        assert!(result.text.contains("- read:"));
        // Should NOT include write tools
        assert!(!result.text.contains("- write:"));
        assert!(!result.text.contains("- edit:"));
    }

    #[test]
    fn system_prompt_role_instructions_appended() {
        let reg = make_registry();
        let role = make_readonly_role();
        let result = test_assemble(&reg, &[], &[], &[], None, Some(&role));
        assert!(result
            .text
            .contains("Review code carefully. Do not modify files."));
    }

    #[test]
    fn system_prompt_role_output_schema_metadata_appended_as_guidance() {
        let reg = make_registry();
        let mut role = make_worker_role();
        role.output_schema = Some(crate::roles::RoleOutputSchema {
            name: "verification-result".into(),
            description: "Structured verifier result".into(),
            required_sections: vec!["status".into(), "commands".into()],
            output_contract: Some("Return status and commands.".into()),
            example: Some("status: passed\ncommands: ...".into()),
            ..crate::roles::RoleOutputSchema::default()
        });
        let result = test_assemble(&reg, &[], &[], &[], None, Some(&role));
        assert!(result.text.contains("Role output schema metadata:"));
        assert!(result.text.contains("- schema: `verification-result`"));
        assert!(result
            .text
            .contains("- required sections: status, commands"));
        assert!(result.text.contains("Return status and commands."));
        assert!(result.text.contains("metadata, not enforced validation"));
    }

    #[test]
    fn system_prompt_worker_role_includes_all_tools() {
        let reg = make_registry();
        let role = make_worker_role();
        let result = test_assemble(&reg, &[], &[], &[], None, Some(&role));
        assert!(result.text.contains("- read:"));
        assert!(result.text.contains("- write:"));
        assert!(result.text.contains("- edit:"));
        assert!(result.text.contains("- bash:"));
    }

    #[test]
    fn system_prompt_no_role_instructions_when_none() {
        let reg = make_registry();
        let role = make_worker_role();
        let result = test_assemble(&reg, &[], &[], &[], None, Some(&role));
        // Worker has no instructions, so the prompt shouldn't have extra instruction text
        let lines: Vec<&str> = result.text.lines().collect();
        let after_tools = lines.iter().position(|l| l.starts_with("- bash:")).unwrap();
        // Next non-empty line after the last tool should be end of identity layer
        // (no instructions appended)
        let remaining = &lines[after_tools + 1..];
        let next_content = remaining.iter().find(|l| !l.is_empty());
        assert!(next_content.is_none() || !next_content.unwrap().contains("Review"));
    }

    // -- Size tracking --

    #[test]
    fn system_prompt_tracks_estimated_tokens() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        assert!(result.estimated_tokens > 0);
        // Rough check: the text is at least ~100 chars, so >= 25 tokens
        assert!(result.estimated_tokens >= 10);
    }

    #[test]
    fn system_prompt_more_layers_means_more_tokens() {
        let reg = make_registry();

        let minimal = test_assemble(&reg, &[], &[], &[], None, None);

        let agents = vec![make_agents_md(
            "Lots of project context here with many words.",
        )];
        let skills = vec![make_skill(
            "rust",
            "Rust conventions",
            "/skills/rust/SKILL.md",
        )];
        let facts = vec![Fact {
            text: "Uses Postgres".into(),
            verified_ago: "1h ago".into(),
        }];

        let full = test_assemble(&reg, &agents, &skills, &facts, None, None);

        assert!(
            full.estimated_tokens > minimal.estimated_tokens,
            "full ({}) should have more tokens than minimal ({})",
            full.estimated_tokens,
            minimal.estimated_tokens
        );
    }

    // -- Full assembly --

    #[test]
    fn system_prompt_all_layers_present() {
        let reg = make_registry();
        let agents = vec![make_agents_md("Be concise.")];
        let skills = vec![make_skill(
            "rust",
            "Rust code conventions",
            "/skills/rust/SKILL.md",
        )];
        let facts = vec![Fact {
            text: "Uses SQLite".into(),
            verified_ago: "30m ago".into(),
        }];
        let task = TaskContext {
            title: "Add caching".into(),
            description: "Add Redis caching layer".into(),
            design: None,
            acceptance: None,
            verify: Some("cargo test cache".into()),
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![Attempt {
                number: 1,
                outcome: "failed".into(),
                summary: "Wrong key format".into(),
            }],
            dependencies: vec![Dependency {
                name: "Config".into(),
                status: "done".into(),
                detail: "src/config.rs".into(),
            }],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };

        let result = test_assemble(&reg, &agents, &skills, &facts, Some(&task), None);

        // All layers present in order
        let identity_pos = result.text.find("You are imp").unwrap();
        let context_pos = result.text.find("# Project Context").unwrap();
        let skills_pos = result.text.find("Available skills").unwrap();
        let facts_pos = result.text.find("Project facts").unwrap();
        let task_pos = result.text.find("## Task").unwrap();

        assert!(identity_pos < context_pos, "identity before context");
        assert!(context_pos < skills_pos, "context before skills");
        assert!(skills_pos < facts_pos, "skills before facts");
        assert!(facts_pos < task_pos, "facts before task");
    }

    #[test]
    fn system_prompt_display_impl() {
        let reg = make_registry();
        let result = test_assemble(&reg, &[], &[], &[], None, None);
        let displayed = format!("{result}");
        assert_eq!(displayed, result.text);
    }

    // -- Layer 6: Agent Memory --

    #[test]
    fn system_prompt_memory_included() {
        let reg = make_registry();
        let mem = "══════════════════\nMEMORY [50% — 100/200]\n══════════════════\nUser runs macOS";
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: Some(mem),
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(result.text.contains("MEMORY"));
        assert!(result.text.contains("User runs macOS"));
    }

    #[test]
    fn system_prompt_user_profile_included() {
        let reg = make_registry();
        let user =
            "══════════════════\nUSER PROFILE [30% — 42/140]\n══════════════════\nPrefers concise";
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: None,
            user_profile: Some(user),
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(result.text.contains("USER PROFILE"));
        assert!(result.text.contains("Prefers concise"));
    }

    #[test]
    fn system_prompt_empty_memory_skipped() {
        let reg = make_registry();
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &[],
            skills: &[],
            facts: &[],
            project_memory_status: None,
            soul: None,
            task: None,
            role: None,
            mode: &AgentMode::Full,
            memory: Some(""),
            user_profile: Some(""),
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });
        assert!(!result.text.contains("MEMORY"));
        assert!(!result.text.contains("USER PROFILE"));
    }

    #[test]
    fn system_prompt_memory_after_all_other_layers() {
        let reg = make_registry();
        let agents = vec![make_agents_md("Project context.")];
        let skills = vec![make_skill("rust", "Rust", "/skills/rust/SKILL.md")];
        let facts = vec![Fact {
            text: "Uses SQLite".into(),
            verified_ago: "1h".into(),
        }];
        let task = TaskContext {
            title: "Fix bug".into(),
            description: "Broken".into(),
            design: None,
            acceptance: None,
            verify: None,
            verify_timeout_secs: None,
            fail_first: false,
            notes: None,
            attempts: vec![],
            dependencies: vec![],
            decisions: vec![],
            context_paths: vec![],
            constraints: vec![],
        };
        let mem = "══════\nMEMORY [50%]\n══════\nSome fact";
        let result = assemble(&AssembleParams {
            tools: &reg,
            agents_md: &agents,
            skills: &skills,
            facts: &facts,
            project_memory_status: None,
            soul: None,
            task: Some(&task),
            role: None,
            mode: &AgentMode::Full,
            memory: Some(mem),
            user_profile: None,
            cwd: None,
            repo_context: None,
            learning_enabled: false,
            guardrail_profile: None,
        });

        let identity_pos = result.text.find("You are imp").unwrap();
        let context_pos = result.text.find("# Project Context").unwrap();
        let facts_pos = result.text.find("Project facts").unwrap();
        let task_pos = result.text.find("## Task").unwrap();
        let memory_pos = result.text.find("MEMORY").unwrap();

        assert!(identity_pos < context_pos);
        assert!(context_pos < facts_pos);
        assert!(facts_pos < task_pos);
        assert!(task_pos < memory_pos, "memory should come after task");
    }
}
