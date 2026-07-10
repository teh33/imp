use imp_llm::{ModelMeta, ModelRegistry, ThinkingLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Role definition from config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoleDef {
    pub model: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub tools: Option<Vec<String>>,
    pub tool_policy: Option<RoleToolPolicy>,
    pub readonly: bool,
    pub instructions: Option<String>,
    pub instruction_set: Vec<String>,
    pub purpose: Option<String>,
    pub prompt_template: Option<String>,
    pub autonomy: Option<RoleAutonomy>,
    pub required_evidence: Vec<EvidenceRequirement>,
    pub verification: Option<RoleVerification>,
    pub model_routing: Option<RoleModelRouting>,
    pub output_schema: Option<RoleOutputSchema>,
    pub child_workflow: Option<ChildWorkflowEligibility>,
}

/// Resolved role ready for use.
#[derive(Debug, Clone)]
pub struct Role {
    pub name: String,
    pub model: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub tool_set: ToolSet,
    pub tool_policy: RoleToolPolicy,
    pub readonly: bool,
    pub instructions: Option<String>,
    pub instruction_set: Vec<String>,
    pub purpose: Option<String>,
    pub prompt_template: Option<String>,
    pub autonomy: RoleAutonomy,
    pub required_evidence: Vec<EvidenceRequirement>,
    pub verification: RoleVerification,
    pub model_routing: RoleModelRouting,
    pub output_schema: Option<RoleOutputSchema>,
    pub child_workflow: ChildWorkflowEligibility,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RoleToolPolicy {
    #[default]
    All,
    Only(Vec<String>),
    AllExcept(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum ToolSet {
    All,
    Only(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleAutonomy {
    pub can_modify_files: bool,
    pub can_run_commands: bool,
    pub can_create_workflows: bool,
    pub can_delegate_child_workflows: bool,
    pub requires_plan_before_write: bool,
    pub stop_on_verification_failure: bool,
    pub max_consecutive_tool_calls: Option<u32>,
}

impl Default for RoleAutonomy {
    fn default() -> Self {
        Self {
            can_modify_files: false,
            can_run_commands: false,
            can_create_workflows: false,
            can_delegate_child_workflows: false,
            requires_plan_before_write: false,
            stop_on_verification_failure: true,
            max_consecutive_tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EvidenceRequirement {
    pub kind: String,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleVerification {
    pub required: bool,
    pub suggested_commands: Vec<String>,
    pub requires_human_review: bool,
    pub accepts_manual_evidence: bool,
}

impl Default for RoleVerification {
    fn default() -> Self {
        Self {
            required: false,
            suggested_commands: Vec::new(),
            requires_human_review: false,
            accepts_manual_evidence: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoleModelClass {
    Fast,
    Cheap,
    HighReasoning,
    Code,
    Review,
    Summarizer,
    LongContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RoleModelRouting {
    pub preferred_model: Option<String>,
    pub fallback_models: Vec<String>,
    pub thinking: Option<ThinkingLevel>,
    pub latency_preference: Option<LatencyPreference>,
    pub cost_preference: Option<CostPreference>,
    pub model_classes: Vec<RoleModelClass>,
    pub capability_hints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatencyPreference {
    Low,
    Balanced,
    Flexible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CostPreference {
    Low,
    Balanced,
    Quality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RoleOutputSchema {
    pub name: String,
    pub description: String,
    pub json_schema_ref: Option<String>,
    pub required_sections: Vec<String>,
    pub output_contract: Option<String>,
    pub example: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChildWorkflowEligibility {
    pub eligible: bool,
    pub can_coordinate_children: bool,
    pub max_children: Option<u32>,
    pub allowed_child_roles: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoleRegistryDef {
    pub roles: BTreeMap<String, RoleDef>,
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RoleRegistry {
    roles: BTreeMap<String, RoleDef>,
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleRegistryError {
    UnknownAliasTarget { alias: String, target: String },
    InvalidRoleName(String),
    EmptyInstructions(String),
    InvalidTool { role: String, tool: String },
    ReadonlyWriteTool { role: String, tool: String },
    InvalidAutonomy { role: String, reason: String },
}

impl std::fmt::Display for RoleRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleRegistryError::UnknownAliasTarget { alias, target } => {
                write!(f, "role alias `{alias}` targets unknown role `{target}`")
            }
            RoleRegistryError::InvalidRoleName(role) => write!(f, "invalid role name `{role}`"),
            RoleRegistryError::EmptyInstructions(role) => {
                write!(
                    f,
                    "role `{role}` must include instructions or an instruction_set"
                )
            }
            RoleRegistryError::InvalidTool { role, tool } => {
                write!(f, "role `{role}` references unknown tool `{tool}`")
            }
            RoleRegistryError::ReadonlyWriteTool { role, tool } => {
                write!(
                    f,
                    "readonly role `{role}` cannot allow write-capable tool `{tool}`"
                )
            }
            RoleRegistryError::InvalidAutonomy { role, reason } => {
                write!(f, "role `{role}` has invalid autonomy: {reason}")
            }
        }
    }
}

impl std::error::Error for RoleRegistryError {}

impl RoleRegistry {
    pub fn from_overrides(
        overrides: impl IntoIterator<Item = (String, RoleDef)>,
    ) -> Result<Self, RoleRegistryError> {
        let mut roles: BTreeMap<String, RoleDef> = builtin_roles()
            .into_iter()
            .map(|(name, def)| (name.to_string(), def))
            .collect();
        for (name, def) in overrides {
            validate_role_name(&name)?;
            roles.insert(name, def);
        }
        let aliases = builtin_aliases();
        let registry = Self { roles, aliases };
        registry.validate()?;
        Ok(registry)
    }

    pub fn role_names(&self) -> impl Iterator<Item = &str> {
        self.roles.keys().map(String::as_str)
    }

    pub fn resolve_def(&self, name: &str) -> Option<&RoleDef> {
        let target = self.aliases.get(name).map(String::as_str).unwrap_or(name);
        self.roles.get(target)
    }

    pub fn resolve(&self, name: &str) -> Option<Role> {
        self.resolve_def(name).map(|def| Role::from_def(name, def))
    }

    pub fn resolve_model_for_role<'a>(
        &self,
        role_name: &str,
        models: &'a ModelRegistry,
    ) -> Option<&'a ModelMeta> {
        let role = self.resolve(role_name)?;
        resolve_role_model(&role, models)
    }

    fn validate(&self) -> Result<(), RoleRegistryError> {
        for (alias, target) in &self.aliases {
            if !self.roles.contains_key(target) && !matches!(alias.as_str(), "worker" | "explorer")
            {
                return Err(RoleRegistryError::UnknownAliasTarget {
                    alias: alias.clone(),
                    target: target.clone(),
                });
            }
        }
        for (name, def) in &self.roles {
            validate_role_name(name)?;
            validate_role_def(name, def)?;
        }
        Ok(())
    }
}

impl RoleRegistryDef {
    pub fn into_registry(self) -> Result<RoleRegistry, RoleRegistryError> {
        let mut registry = RoleRegistry::from_overrides(self.roles)?;
        registry.aliases.extend(self.aliases);
        registry.validate()?;
        Ok(registry)
    }
}

impl Role {
    /// Create a role from a definition.
    pub fn from_def(name: &str, def: &RoleDef) -> Self {
        let tool_set = match &def.tools {
            Some(tools) => ToolSet::Only(tools.clone()),
            None if def.readonly => readonly_tool_set(),
            None => ToolSet::All,
        };
        let tool_policy = def.tool_policy.clone().unwrap_or_else(|| match &def.tools {
            Some(tools) => RoleToolPolicy::Only(tools.clone()),
            None if def.readonly => RoleToolPolicy::Only(readonly_tools()),
            None => RoleToolPolicy::All,
        });
        let mut instruction_set = def.instruction_set.clone();
        if let Some(instructions) = &def.instructions {
            if !instructions.is_empty() && instruction_set.is_empty() {
                instruction_set.push(instructions.clone());
            }
        }

        Self {
            name: name.to_string(),
            model: def.model.clone(),
            thinking_level: def.thinking,
            tool_set,
            tool_policy,
            readonly: def.readonly,
            instructions: def.instructions.clone(),
            instruction_set,
            purpose: def.purpose.clone(),
            prompt_template: def.prompt_template.clone(),
            autonomy: def
                .autonomy
                .clone()
                .unwrap_or_else(|| default_autonomy(def.readonly)),
            required_evidence: def.required_evidence.clone(),
            verification: def.verification.clone().unwrap_or_default(),
            model_routing: def
                .model_routing
                .clone()
                .unwrap_or_else(|| RoleModelRouting {
                    preferred_model: def.model.clone(),
                    thinking: def.thinking,
                    ..RoleModelRouting::default()
                }),
            output_schema: def.output_schema.clone(),
            child_workflow: def.child_workflow.clone().unwrap_or_default(),
        }
    }
}

fn readonly_tools() -> Vec<String> {
    vec!["read".into(), "scan".into(), "web".into()]
}

fn readonly_tool_set() -> ToolSet {
    ToolSet::Only(readonly_tools())
}

fn default_autonomy(readonly: bool) -> RoleAutonomy {
    RoleAutonomy {
        can_modify_files: !readonly,
        can_run_commands: !readonly,
        stop_on_verification_failure: true,
        ..RoleAutonomy::default()
    }
}

fn evidence(kind: &str, description: &str) -> EvidenceRequirement {
    EvidenceRequirement {
        kind: kind.into(),
        required: true,
        description: description.into(),
    }
}

fn routing(thinking: ThinkingLevel, hints: &[&str]) -> RoleModelRouting {
    let model_classes = hints
        .iter()
        .filter_map(|hint| match *hint {
            "planning" => Some(RoleModelClass::HighReasoning),
            "code-editing" => Some(RoleModelClass::Code),
            "test-debugging" => Some(RoleModelClass::HighReasoning),
            "review" | "reasoning" => Some(RoleModelClass::Review),
            "research" | "summarization" => Some(RoleModelClass::Summarizer),
            "long-context" => Some(RoleModelClass::LongContext),
            _ => None,
        })
        .collect();
    RoleModelRouting {
        thinking: Some(thinking),
        latency_preference: Some(LatencyPreference::Balanced),
        cost_preference: Some(CostPreference::Balanced),
        model_classes,
        capability_hints: hints.iter().map(|hint| (*hint).into()).collect(),
        ..RoleModelRouting::default()
    }
}

fn output_schema(name: &str, sections: &[&str]) -> RoleOutputSchema {
    RoleOutputSchema {
        name: name.into(),
        description: format!("Structured {name} role output metadata"),
        required_sections: sections.iter().map(|section| (*section).into()).collect(),
        output_contract: Some(format!(
            "Include these sections in the final role output: {}.",
            sections.join(", ")
        )),
        example: Some(
            sections
                .iter()
                .map(|section| format!("{section}: ..."))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        json_schema_ref: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn role(
    purpose: &str,
    readonly: bool,
    tools: Vec<String>,
    instructions: &[&str],
    autonomy: RoleAutonomy,
    required_evidence: Vec<EvidenceRequirement>,
    verification: RoleVerification,
    model_routing: RoleModelRouting,
    output_schema: RoleOutputSchema,
    child_workflow: ChildWorkflowEligibility,
) -> RoleDef {
    RoleDef {
        thinking: model_routing.thinking,
        tools: Some(tools.clone()),
        tool_policy: Some(RoleToolPolicy::Only(tools)),
        readonly,
        instructions: instructions
            .first()
            .map(|instruction| (*instruction).into()),
        instruction_set: instructions
            .iter()
            .map(|instruction| (*instruction).into())
            .collect(),
        purpose: Some(purpose.into()),
        autonomy: Some(autonomy),
        required_evidence,
        verification: Some(verification),
        model_routing: Some(model_routing),
        output_schema: Some(output_schema),
        child_workflow: Some(child_workflow),
        ..RoleDef::default()
    }
}

fn resolve_role_model<'a>(role: &Role, models: &'a ModelRegistry) -> Option<&'a ModelMeta> {
    if let Some(model) = role
        .model_routing
        .preferred_model
        .as_deref()
        .or(role.model.as_deref())
    {
        return models.find_by_alias(model);
    }
    for fallback in &role.model_routing.fallback_models {
        if let Some(model) = models.find_by_alias(fallback) {
            return Some(model);
        }
    }
    models
        .list()
        .iter()
        .filter(|model| model.capabilities.tool_use)
        .filter(|model| model_matches_role_routing(model, &role.model_routing))
        .max_by_key(|model| role_model_score(model, &role.model_routing))
}

fn model_matches_role_routing(model: &ModelMeta, routing: &RoleModelRouting) -> bool {
    if routing.model_classes.is_empty() && routing.capability_hints.is_empty() {
        return true;
    }
    routing
        .model_classes
        .iter()
        .all(|class| model_matches_class(model, *class))
        && routing
            .capability_hints
            .iter()
            .all(|hint| model_matches_capability_hint(model, hint))
}

fn role_model_score(model: &ModelMeta, routing: &RoleModelRouting) -> i64 {
    let mut score = model.context_window as i64 / 100_000;
    if model.capabilities.reasoning {
        score += 10;
    }
    if matches!(routing.cost_preference, Some(CostPreference::Low)) {
        score -= ((model.pricing.input_per_mtok + model.pricing.output_per_mtok) * 10.0) as i64;
    }
    if matches!(routing.latency_preference, Some(LatencyPreference::Low)) {
        score -= model.context_window as i64 / 250_000;
    }
    score
}

fn model_matches_class(model: &ModelMeta, class: RoleModelClass) -> bool {
    match class {
        RoleModelClass::Fast => {
            model.context_window <= 512_000
                || model.id.contains("haiku")
                || model.id.contains("mini")
                || model.id.contains("flash")
                || model.id.contains("spark")
        }
        RoleModelClass::Cheap => {
            model.pricing.input_per_mtok <= 1.5
                || model.id.contains("haiku")
                || model.id.contains("mini")
                || model.id.contains("nano")
                || model.id.contains("flash")
        }
        RoleModelClass::HighReasoning => model.capabilities.reasoning,
        RoleModelClass::Code => {
            model.capabilities.tool_use
                && (model.id.contains("codex")
                    || model.id.contains("sonnet")
                    || model.id.contains("kimi")
                    || model.id.contains("gpt"))
        }
        RoleModelClass::Review => {
            model.capabilities.reasoning || model.id.contains("sonnet") || model.id.contains("opus")
        }
        RoleModelClass::Summarizer => model.context_window >= 128_000,
        RoleModelClass::LongContext => model.context_window >= 512_000,
    }
}

fn model_matches_capability_hint(model: &ModelMeta, hint: &str) -> bool {
    match hint {
        "planning" | "review" | "reasoning" => model.capabilities.reasoning,
        "code-editing" => model_matches_class(model, RoleModelClass::Code),
        "test-debugging" => model.capabilities.reasoning,
        "research" | "summarization" => model_matches_class(model, RoleModelClass::Summarizer),
        "long-context" => model_matches_class(model, RoleModelClass::LongContext),
        "structured-output" => model.capabilities.tool_use,
        _ => true,
    }
}

fn builtin_aliases() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("worker".into(), "coder".into()),
        ("explorer".into(), "researcher".into()),
    ])
}

fn known_tool(name: &str) -> bool {
    matches!(
        name,
        "ask_user"
            | "audit_scan"
            | "bash"
            | "color_palette"
            | "edit"
            | "extend"
            | "git"
            | "task"
            | "workflow"
            | "openrouter_secret_run"
            | "read"
            | "scan"
            | "web"
            | "work"
            | "write"
    )
}

fn write_capable_tool(name: &str) -> bool {
    matches!(name, "edit" | "extend" | "write")
}

fn validate_role_name(name: &str) -> Result<(), RoleRegistryError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(RoleRegistryError::InvalidRoleName(name.into()));
    }
    Ok(())
}

fn validate_role_def(name: &str, def: &RoleDef) -> Result<(), RoleRegistryError> {
    if def.instructions.as_deref().unwrap_or_default().is_empty() && def.instruction_set.is_empty()
    {
        return Err(RoleRegistryError::EmptyInstructions(name.into()));
    }
    let tools = match (&def.tool_policy, &def.tools) {
        (Some(RoleToolPolicy::Only(tools)), _) | (_, Some(tools)) => tools.as_slice(),
        (Some(RoleToolPolicy::AllExcept(tools)), _) => tools.as_slice(),
        _ => &[],
    };
    for tool in tools {
        if !known_tool(tool) {
            return Err(RoleRegistryError::InvalidTool {
                role: name.into(),
                tool: tool.clone(),
            });
        }
        if def.readonly && write_capable_tool(tool) {
            return Err(RoleRegistryError::ReadonlyWriteTool {
                role: name.into(),
                tool: tool.clone(),
            });
        }
    }
    if let Some(autonomy) = &def.autonomy {
        if def.readonly && autonomy.can_modify_files {
            return Err(RoleRegistryError::InvalidAutonomy {
                role: name.into(),
                reason: "readonly role cannot modify files".into(),
            });
        }
        if !autonomy.can_run_commands && def.readonly && tools.iter().any(|tool| tool == "bash") {
            return Err(RoleRegistryError::InvalidAutonomy {
                role: name.into(),
                reason: "readonly role with bash verifier access must set can_run_commands".into(),
            });
        }
    }
    Ok(())
}

/// Built-in role definitions.
pub fn builtin_roles() -> Vec<(&'static str, RoleDef)> {
    let read_tools = readonly_tools();
    vec![
        (
            "planner",
            role(
                "Decompose work into safe, verifiable steps.",
                true,
                read_tools.clone(),
                &[
                    "Plan the work before implementation. Do not edit product files.",
                    "Identify acceptance criteria, risks, dependencies, and verification gates.",
                ],
                RoleAutonomy { can_create_workflows: true, can_delegate_child_workflows: true, requires_plan_before_write: true, ..RoleAutonomy::default() },
                vec![evidence("plan", "Proposed implementation plan"), evidence("acceptance-criteria", "Acceptance criteria and verification gates"), evidence("risk-notes", "Risks, assumptions, and blockers")],
                RoleVerification { accepts_manual_evidence: true, ..RoleVerification::default() },
                routing(ThinkingLevel::High, &["planning", "long-context", "structured-output"]),
                output_schema("plan", &["goals", "tasks", "risks", "verification"]),
                ChildWorkflowEligibility { eligible: true, can_coordinate_children: true, max_children: Some(8), allowed_child_roles: vec!["coder".into(), "verifier".into(), "reviewer".into(), "researcher".into()] },
            ),
        ),
        (
            "coder",
            role(
                "Implement focused code changes within policy.",
                false,
                vec!["read".into(), "scan".into(), "edit".into(), "write".into(), "bash".into(), "git".into(), "workflow".into()],
                &[
                    "Make the smallest coherent code change that satisfies the task.",
                    "Run narrow verification for changed behavior and report unresolved concerns.",
                ],
                RoleAutonomy { can_modify_files: true, can_run_commands: true, stop_on_verification_failure: true, max_consecutive_tool_calls: Some(20), ..RoleAutonomy::default() },
                vec![evidence("diff-summary", "Files changed and rationale"), evidence("verification-result", "Verification command and result"), evidence("open-concerns", "Remaining risks or blockers")],
                RoleVerification { required: true, accepts_manual_evidence: true, ..RoleVerification::default() },
                routing(ThinkingLevel::Medium, &["code-editing", "test-debugging"]),
                output_schema("implementation-summary", &["changed", "verified", "concerns"]),
                ChildWorkflowEligibility { eligible: true, max_children: Some(1), ..ChildWorkflowEligibility::default() },
            ),
        ),
        (
            "verifier",
            role(
                "Independently check whether acceptance criteria are satisfied.",
                true,
                vec!["read".into(), "scan".into(), "bash".into(), "git".into()],
                &[
                    "Run only declared or clearly relevant verification commands.",
                    "Do not fix failures; report pass, fail, or blocked with evidence.",
                ],
                RoleAutonomy { can_run_commands: true, stop_on_verification_failure: true, max_consecutive_tool_calls: Some(10), ..RoleAutonomy::default() },
                vec![evidence("test-output", "Commands run and output refs"), evidence("verification-result", "Pass/fail/blocked status"), evidence("failure-excerpts", "Relevant failure excerpts")],
                RoleVerification { required: true, accepts_manual_evidence: false, ..RoleVerification::default() },
                routing(ThinkingLevel::Medium, &["test-debugging", "review", "structured-output"]),
                output_schema("verification-result", &["status", "commands", "evidence", "failures"]),
                ChildWorkflowEligibility { eligible: true, max_children: Some(1), ..ChildWorkflowEligibility::default() },
            ),
        ),
        (
            "reviewer",
            role(
                "Review changes for correctness, maintainability, safety, and product fit.",
                true,
                read_tools.clone(),
                &[
                    "Review the diff and relevant context without modifying files.",
                    "Prioritize actionable findings by severity and avoid speculative churn.",
                ],
                RoleAutonomy { stop_on_verification_failure: true, max_consecutive_tool_calls: Some(12), ..RoleAutonomy::default() },
                vec![evidence("review-findings", "Findings with severity and evidence"), evidence("risk-notes", "Risks and follow-up recommendations")],
                RoleVerification { requires_human_review: false, accepts_manual_evidence: true, ..RoleVerification::default() },
                routing(ThinkingLevel::High, &["review", "reasoning"]),
                output_schema("review-findings", &["findings", "positives", "risks"]),
                ChildWorkflowEligibility { eligible: true, max_children: Some(1), ..ChildWorkflowEligibility::default() },
            ),
        ),
        (
            "researcher",
            role(
                "Gather and summarize external or repository context with source labels.",
                true,
                read_tools.clone(),
                &[
                    "Label external or untrusted sources and cite evidence.",
                    "Summarize findings, confidence, and unresolved questions without editing files.",
                ],
                RoleAutonomy { max_consecutive_tool_calls: Some(12), ..RoleAutonomy::default() },
                vec![evidence("source-citations", "Sources consulted and citations"), evidence("research-summary", "Findings and confidence"), evidence("trust-notes", "Trust labels for low-trust content")],
                RoleVerification { accepts_manual_evidence: true, ..RoleVerification::default() },
                routing(ThinkingLevel::Medium, &["research", "summarization", "long-context"]),
                output_schema("research-summary", &["findings", "citations", "confidence", "questions"]),
                ChildWorkflowEligibility { eligible: true, max_children: Some(1), ..ChildWorkflowEligibility::default() },
            ),
        ),
        (
            "integrator",
            role(
                "Combine role outputs into a coherent final result and resolve integration concerns.",
                false,
                vec!["read".into(), "scan".into(), "edit".into(), "write".into(), "bash".into(), "git".into(), "workflow".into()],
                &[
                    "Synthesize child outputs, preserve decisions, and avoid broadening scope.",
                    "Ensure required verification gates are present or clearly explain blockers.",
                ],
                RoleAutonomy { can_modify_files: true, can_run_commands: true, can_create_workflows: true, can_delegate_child_workflows: true, stop_on_verification_failure: true, max_consecutive_tool_calls: Some(16), ..RoleAutonomy::default() },
                vec![evidence("integration-summary", "Combined outcome and resolved conflicts"), evidence("decision-log", "Decisions made during integration"), evidence("verification-result", "Final verification state")],
                RoleVerification { required: true, accepts_manual_evidence: true, ..RoleVerification::default() },
                routing(ThinkingLevel::Medium, &["long-context", "synthesis", "code-editing"]),
                output_schema("integration-summary", &["summary", "decisions", "conflicts", "verification"]),
                ChildWorkflowEligibility { eligible: true, can_coordinate_children: true, max_children: Some(8), allowed_child_roles: vec!["planner".into(), "coder".into(), "verifier".into(), "reviewer".into(), "researcher".into()] },
            ),
        ),
        ("worker", alias_def("coder")),
        ("explorer", alias_def("researcher")),
    ]
}

fn alias_def(target: &str) -> RoleDef {
    RoleDef {
        instructions: Some(format!("Compatibility alias for `{target}`.")),
        purpose: Some(format!("Compatibility alias for `{target}`.")),
        ..RoleDef::default()
    }
}

#[cfg(test)]
#[path = "roles/tests.rs"]
mod tests;
