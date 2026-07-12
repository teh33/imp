use std::fmt;

use crate::config::AgentMode;
use crate::context::estimate_tokens;
use crate::guardrails::{self, GuardrailProfile};
use crate::resources::{AgentsMd, Skill};
use crate::roles::Role;
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
    pub task: Option<&'a TaskContext>,
    pub role: Option<&'a Role>,
    pub mode: &'a AgentMode,
    pub cwd: Option<&'a std::path::Path>,
    pub repo_context: Option<&'a crate::repo_intelligence::RepoContextSummary>,
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
    parts.push(identity_layer(p.tools, p.role, p.mode));

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

    let text = parts.join("\n\n");
    let estimated_tokens = estimate_tokens(&text);

    AssembledPrompt {
        text,
        estimated_tokens,
    }
}

fn identity_layer(tools: &ToolRegistry, role: Option<&Role>, mode: &AgentMode) -> String {
    let mut s = String::new();
    s.push_str("You are imp, a professional coding agent.");
    s.push_str("\n\nAvailable tools:\n");

    let mut defs = match role {
        Some(r) if r.readonly => tools.readonly_definitions(),
        _ => tools.definitions_for_mode(mode),
    };
    // `task` is a dormant planning tool. It is advertised dynamically when
    // the session enters planning mode, not as a default one-shot tool.
    defs.retain(|definition| definition.name != "task");

    for def in &defs {
        s.push_str(&format!("- {}: {}\n", def.name, def.description));
    }

    s.push_str("\nTool routing:\n");
    s.push_str("- For code lookup, use `scan` for structure and `rg` for raw text; avoid unpruned `find`. Use `bash` for builds, tests, scripts, and package managers.\n");
    if defs.iter().any(|def| def.name == "git") {
        s.push_str("- Use `git` for local repo/worktree operations; use `bash` for uncovered git commands.\n");
    }
    if !matches!(mode, AgentMode::Full) && defs.iter().any(|def| def.name == "workflow") {
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
    let mut s = String::from("# Project Instructions\n\n");
    s.push_str(
        "Instruction files are ordered from broadest to most specific. Later files override earlier files when they conflict. Direct user and system instructions still take precedence over these files.\n",
    );

    for (index, agent) in agents.iter().enumerate() {
        let precedence = index + 1;
        s.push_str("\n## Instruction File ");
        s.push_str(&precedence.to_string());
        s.push_str(": ");
        s.push_str(&agent.path.display().to_string());
        s.push('\n');
        s.push_str("```markdown\n");
        s.push_str(agent.content.trim());
        s.push_str("\n```\n");
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
#[path = "system_prompt/tests.rs"]
mod tests;
