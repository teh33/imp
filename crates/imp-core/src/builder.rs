use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use imp_llm::Model;

use crate::agent::{Agent, AgentHandle};
use crate::config::{Config, LuaCapabilityPolicy};
use crate::error::Result;
use crate::policy::RunPolicy;
use crate::resources;
use crate::roles::{Role, RoleToolPolicy};
use crate::system_prompt::{self, Fact, TaskContext};
use crate::tools::{LuaToolLoader, ToolRegistry};
use crate::workflow::{
    AutonomyMode, ImplicitWorkflowContractInput, VerificationGate, VerificationRequirement,
    WorkflowContract, WorktreeRunMetadata, WorktreeRunPlan,
};

#[derive(Clone)]
pub struct PromptContext {
    pub facts: Vec<Fact>,
    pub project_memory_status: Option<String>,
}

impl PromptContext {
    pub fn from_facts(facts: Vec<Fact>) -> Self {
        Self {
            facts,
            project_memory_status: None,
        }
    }
}

/// Builder for creating a fully wired [`Agent`] from config and context.
///
/// Handles resource discovery, hook loading, system prompt assembly, and tool
/// registration so callers don't need to repeat this boilerplate.
pub struct AgentBuilder {
    config: Config,
    cwd: PathBuf,
    model: Model,
    api_key: String,
    role: Option<Role>,
    task: Option<TaskContext>,
    facts: Vec<Fact>,
    /// Override the assembled system prompt entirely.
    system_prompt_override: Option<String>,
    /// Additional tool registrar called after native tools are registered.
    #[allow(clippy::type_complexity)]
    extra_tools: Option<Box<dyn FnOnce(&mut ToolRegistry) + Send>>,
    /// Skip native and Lua tool registration before system prompt assembly.
    no_tools: bool,
    /// Preloaded Lua extension tool registrar.
    preloaded_lua_tools: Option<ToolRegistry>,
    /// Lua extension tool loader — called after native and extra tools.
    ///
    /// The imp-lua crate provides the actual implementation; the binary
    /// crate wires it in to avoid a cyclic dependency between imp-core
    /// and imp-lua.
    #[allow(clippy::type_complexity)]
    lua_tool_loader: Option<LuaToolLoader>,
    /// Per-run tool/write policy layered on top of AgentMode.
    run_policy: RunPolicy,
    /// Preloaded workflow prompt context; avoids duplicate workflow reads for worker mode.
    preloaded_prompt_context: Option<PromptContext>,
    /// Optional workflow contract override. If absent, build creates an implicit contract.
    pub verification_gates: Vec<VerificationGate>,
    workflow_contract: Option<WorkflowContract>,
    worktree_run_plan: Option<WorktreeRunPlan>,
    worktree_run_metadata: Option<WorktreeRunMetadata>,
}

impl AgentBuilder {
    /// Create a new builder.
    pub fn new(config: Config, cwd: PathBuf, model: Model, api_key: String) -> Self {
        Self {
            config,
            cwd,
            model,
            api_key,
            role: None,
            task: None,
            facts: Vec::new(),
            system_prompt_override: None,
            extra_tools: None,
            no_tools: false,
            preloaded_lua_tools: None,
            preloaded_prompt_context: None,
            lua_tool_loader: None,
            run_policy: RunPolicy::default(),
            verification_gates: Vec::new(),
            workflow_contract: None,
            worktree_run_plan: None,
            worktree_run_metadata: None,
        }
    }

    /// Set the role for this agent.
    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// Set the task context (headless/task mode — Layer 5 of the system prompt).
    pub fn task(mut self, task: TaskContext) -> Self {
        self.task = Some(task);
        self
    }

    /// Set task-specific facts to inject into the system prompt.
    pub fn facts(mut self, facts: Vec<Fact>) -> Self {
        self.facts = facts;
        self
    }

    /// Override the assembled system prompt with a custom string.
    /// When set, resource discovery and assembly are skipped.
    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt_override = Some(prompt);
        self
    }

    /// Register additional tools after the native tools are registered.
    pub fn extra_tools<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut ToolRegistry) + Send + 'static,
    {
        self.extra_tools = Some(Box::new(f));
        self
    }

    /// Skip native and Lua tool registration before system prompt assembly.
    pub fn no_tools(mut self, no_tools: bool) -> Self {
        self.no_tools = no_tools;
        self
    }

    /// Register a Lua extension tool loader.
    ///
    /// The provided closure should discover `.lua` extensions, create a
    /// `LuaRuntime`, and register the resulting tools onto the registry.
    /// This is called after native + extra tools are registered but before
    /// mode filtering.
    ///
    /// The binary crate typically calls this with a closure that invokes
    /// `imp_lua::load_lua_extensions()`.
    pub fn preloaded_lua_tools(mut self, tools: ToolRegistry) -> Self {
        self.preloaded_lua_tools = Some(tools);
        self
    }

    pub fn lua_tool_loader<F>(mut self, f: F) -> Self
    where
        F: Fn(&LuaCapabilityPolicy, &mut ToolRegistry) + Send + Sync + 'static,
    {
        self.lua_tool_loader = Some(Arc::new(f));
        self
    }

    /// Apply a per-run policy on top of the configured agent mode.
    pub fn run_policy(mut self, policy: RunPolicy) -> Self {
        self.run_policy = policy;
        self
    }

    /// Add a verification gate to the agent run.
    pub fn verification_gate(mut self, gate: VerificationGate) -> Self {
        self.verification_gates.push(gate);
        self
    }

    /// Add verification gates to the agent run.
    pub fn verification_gates<I>(mut self, gates: I) -> Self
    where
        I: IntoIterator<Item = VerificationGate>,
    {
        self.verification_gates.extend(gates);
        self
    }

    /// Add a command verification gate to the agent run.
    pub fn verify_command(mut self, command: impl Into<String>, required: bool) -> Self {
        let requirement = VerificationRequirement {
            name: None,
            kind: crate::workflow::VerificationRequirementKind::Command {
                command: command.into(),
            },
            required,
        };
        let gate = VerificationGate::from_requirement(self.verification_gates.len(), &requirement);
        self.verification_gates.push(gate);
        self
    }

    /// Use preloaded workflow prompt context instead of loading it during build.
    pub fn preloaded_prompt_context(mut self, context: PromptContext) -> Self {
        self.preloaded_prompt_context = Some(context);
        self
    }

    /// Override the implicit workflow contract for this agent run.
    pub fn workflow_contract(mut self, contract: WorkflowContract) -> Self {
        self.workflow_contract = Some(contract);
        self
    }

    /// Attach worktree-auto execution metadata and switch the build cwd/scope to the worktree.
    pub fn worktree_run(mut self, plan: WorktreeRunPlan, metadata: WorktreeRunMetadata) -> Self {
        self.cwd = metadata.worktree_path.clone();
        let mut contract = self.workflow_contract.unwrap_or_else(|| {
            WorkflowContract::implicit_from(
                ImplicitWorkflowContractInput::prompt("").cwd(&self.cwd),
            )
        });
        contract.workspace_scope = plan.workspace_scope();
        contract.autonomy_mode = AutonomyMode::WorktreeAuto;
        self.workflow_contract = Some(contract);
        self.worktree_run_plan = Some(plan);
        self.worktree_run_metadata = Some(metadata);
        self
    }

    /// Set the autonomy mode on the implicit workflow contract.
    pub fn autonomy_mode(mut self, mode: AutonomyMode) -> Self {
        let mut contract = self.workflow_contract.unwrap_or_else(|| {
            WorkflowContract::implicit_from(
                ImplicitWorkflowContractInput::prompt("").cwd(&self.cwd),
            )
        });
        contract.autonomy_mode = mode;
        self.workflow_contract = Some(contract);
        self
    }

    fn apply_role_to_workflow_contract(&mut self) {
        let Some(role) = self.role.as_ref() else {
            return;
        };
        let contract = self.workflow_contract.get_or_insert_with(|| {
            WorkflowContract::implicit_from(
                ImplicitWorkflowContractInput::prompt("").cwd(&self.cwd),
            )
        });
        if contract.title.is_none() {
            contract.title = Some(format!("{} role workflow", role.name));
        }
        for command in &role.verification.suggested_commands {
            contract
                .required_verification
                .push(VerificationRequirement::command(command.clone()));
        }
        for evidence in &role.required_evidence {
            let label = if evidence.description.is_empty() {
                evidence.kind.clone()
            } else {
                format!("{}: {}", evidence.kind, evidence.description)
            };
            if evidence.required {
                contract
                    .closeout_criteria
                    .criteria
                    .push(format!("required evidence: {label}"));
            } else {
                contract
                    .closeout_criteria
                    .criteria
                    .push(format!("optional evidence: {label}"));
            }
        }
        match &role.tool_policy {
            RoleToolPolicy::All => {}
            RoleToolPolicy::Only(tools) => {
                for tool in tools {
                    contract.tool_permissions.allowed_tools.insert(tool.clone());
                }
            }
            RoleToolPolicy::AllExcept(tools) => {
                for tool in tools {
                    contract.tool_permissions.denied_tools.insert(tool.clone());
                }
            }
        }
    }

    /// Build the agent, wiring config → thresholds, hooks, resources, and tools.
    ///
    /// Returns `(Agent, AgentHandle)` ready for use.
    pub fn build(mut self) -> Result<(Agent, AgentHandle)> {
        self.apply_role_to_workflow_contract();
        let build_started = Instant::now();
        let trace_path = std::env::var_os("IMP_TUI_TRACE").map(PathBuf::from);
        let trace_phase = |phase: &str, started: Instant| {
            if let Some(path) = trace_path.as_ref() {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(
                        file,
                        "{} agent_builder_phase phase={} duration_ms={}",
                        imp_llm::now(),
                        phase,
                        started.elapsed().as_millis()
                    );
                }
            }
        };

        let default_subagent_model = self
            .config
            .subagent_model
            .clone()
            .unwrap_or_else(|| self.model.meta.id.clone());
        let (mut agent, handle) = Agent::new(self.model, self.cwd.clone());
        agent.api_key = self.api_key;
        if let Some(thinking) = self.config.thinking {
            agent.thinking_level = thinking;
        }
        if let Some(max_tokens) = self.config.max_tokens {
            agent.max_tokens = Some(max_tokens);
        }
        agent.context_config = self.config.context.clone();
        if let Some(ref role) = self.role {
            if let Some(thinking) = role.model_routing.thinking.or(role.thinking_level) {
                agent.thinking_level = thinking;
            }
            agent.role = Some(role.clone());
        }
        agent.hooks.load_from_config(self.config.hooks.clone());
        agent.mode = self.config.mode;
        agent.guardrail_config = self.config.guardrails.clone();
        agent.guardrail_profile = if self.config.guardrails.is_enabled() {
            Some(self.config.guardrails.resolve_effective_profile(&self.cwd))
        } else {
            None
        };
        agent.read_max_lines = self.config.ui.read_max_lines;
        agent.continue_policy = self.config.ui.continue_policy;
        agent.config = Arc::new(self.config.clone());
        agent.run_policy = self.run_policy;
        agent.verification_gates = self.verification_gates;
        if let Some(contract) = self.workflow_contract.clone() {
            agent.set_workflow_contract(contract);
        }
        agent.worktree_run_metadata = self.worktree_run_metadata;
        agent.lua_tool_loader = self.lua_tool_loader.clone();

        let phase_started = Instant::now();
        if !self.no_tools {
            register_native_tools_with_task_state(
                &mut agent.tools,
                Arc::clone(&agent.task_state),
                Some(default_subagent_model),
            );
            register_browser_tool(&mut agent.tools, &self.config);
        }
        if let Some(extra) = self.extra_tools {
            extra(&mut agent.tools);
        }
        trace_phase("native_extra_tools", phase_started);

        let phase_started = Instant::now();
        if self.no_tools {
            // Keep the registry empty before prompt assembly so no-tools runs do
            // not pay for stale tool descriptions in the system prompt.
        } else if let Some(preloaded_lua_tools) = self.preloaded_lua_tools {
            agent.tools.extend(preloaded_lua_tools);
        } else if let Some(lua_loader) = self.lua_tool_loader {
            let lua_policy = self.config.lua.resolve_policy(agent.mode);
            lua_loader(&lua_policy, &mut agent.tools);
        }
        trace_phase("lua_tools", phase_started);

        let phase_started = Instant::now();
        if !self.no_tools && agent.mode != crate::config::AgentMode::Full {
            let mode = agent.mode;
            agent.tools.retain(|name| mode.allows_tool(name));
        }
        if let Some(role) = &agent.role {
            apply_role_tool_policy(&mut agent.tools, role);
        }
        trace_phase("mode_filter", phase_started);

        let phase_started = Instant::now();
        agent.system_prompt = if let Some(prompt) = self.system_prompt_override {
            prompt
        } else {
            let user_config_dir = Config::user_config_dir();
            let resource_started = Instant::now();
            let agents_md = resources::discover_agents_md(&self.cwd, &user_config_dir);
            let skills = resources::discover_skills(&self.cwd, &user_config_dir);
            trace_phase("resources_discovery", resource_started);
            agent.set_workflow_skill_available(skills.iter().any(|skill| skill.name == "workflow"));
            agent.set_workflow_basics_skill_available(
                skills.iter().any(|skill| skill.name == "workflow-basics"),
            );
            agent.set_workflow_delegation_skill_available(
                skills
                    .iter()
                    .any(|skill| skill.name == "workflow-delegation"),
            );

            let prompt_context_started = Instant::now();
            let prompt_context = if self.facts.is_empty() {
                self.preloaded_prompt_context
                    .clone()
                    .unwrap_or_else(|| PromptContext::from_facts(Vec::new()))
            } else {
                PromptContext::from_facts(self.facts.clone())
            };
            trace_phase("prompt_context", prompt_context_started);

            let repo_context_started = Instant::now();
            let repo_context = crate::repo_intelligence::summarize_repo_context_cached(&self.cwd)
                .ok()
                .flatten();
            trace_phase("repo_intelligence_context", repo_context_started);

            let assemble_started = Instant::now();
            let prompt = system_prompt::assemble(&system_prompt::AssembleParams {
                tools: &agent.tools,
                agents_md: &agents_md,
                skills: &skills,
                facts: &prompt_context.facts,
                project_memory_status: prompt_context.project_memory_status.as_deref(),
                soul: resources::discover_soul(&self.cwd, &user_config_dir).as_ref(),
                task: self.task.as_ref(),
                role: self.role.as_ref(),
                mode: &agent.mode,
                memory: None,
                user_profile: None,
                cwd: Some(&self.cwd),
                repo_context: repo_context.as_ref(),
                learning_enabled: false,
                guardrail_profile: agent.guardrail_profile,
            })
            .text;
            trace_phase("system_prompt_assemble", assemble_started);
            prompt
        };
        trace_phase("system_prompt_total", phase_started);
        if let Some(path) = trace_path.as_ref() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(
                    file,
                    "{} agent_builder_total duration_ms={}",
                    imp_llm::now(),
                    build_started.elapsed().as_millis()
                );
            }
        }
        Ok((agent, handle))
    }
}

fn apply_role_tool_policy(tools: &mut ToolRegistry, role: &Role) {
    match &role.tool_policy {
        RoleToolPolicy::All => {}
        RoleToolPolicy::Only(allowed) => {
            tools.retain(|name| allowed.iter().any(|tool| tool == name))
        }
        RoleToolPolicy::AllExcept(denied) => {
            tools.retain(|name| !denied.iter().any(|tool| tool == name))
        }
    }
}

/// Register the standard set of native tools onto a tool registry.
///
/// This is the canonical list — update here when adding or removing tools.
pub fn register_native_tools(tools: &mut ToolRegistry) {
    register_native_tools_with_task_state(
        tools,
        Arc::new(std::sync::Mutex::new(
            crate::agent::task_state::SessionTaskState::default(),
        )),
        None,
    );
}

fn register_browser_tool(tools: &mut ToolRegistry, config: &Config) {
    if config.browser.enabled {
        tools.register(Arc::new(crate::tools::browser::BrowserTool::new(
            config.browser.clone(),
        )));
    }
}

fn register_native_tools_with_task_state(
    tools: &mut ToolRegistry,
    task_state: Arc<std::sync::Mutex<crate::agent::task_state::SessionTaskState>>,
    default_subagent_model: Option<String>,
) {
    use crate::tools::{
        ask::AskTool, bash::BashTool, edit::EditTool, git::GitTool, read::ReadTool, scan::ScanTool,
        subagent::SubagentTool, task::TaskTool, web::WebTool, workflow::WorkflowTool,
        write::WriteTool,
    };

    tools.register(Arc::new(AskTool));
    tools.register(Arc::new(BashTool::canonical()));
    tools.register(Arc::new(EditTool));
    tools.register(Arc::new(GitTool));
    tools.register(Arc::new(ReadTool));
    tools.register(Arc::new(WriteTool));
    tools.register(Arc::new(ScanTool));
    tools.register(Arc::new(WebTool));
    tools.register(Arc::new(SubagentTool::with_default_model(
        default_subagent_model,
    )));
    tools.register(Arc::new(TaskTool::new(Arc::clone(&task_state))));
    tools.register(Arc::new(WorkflowTool));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_core::Stream;
    use imp_llm::{
        auth::{ApiKey, AuthStore},
        model::{Capabilities, ModelMeta, ModelPricing},
        provider::Provider,
        Context, Model, RequestOptions, StreamEvent,
    };

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        fn stream(
            &self,
            _model: &Model,
            _context: Context,
            _options: RequestOptions,
            _api_key: &str,
        ) -> Pin<Box<dyn Stream<Item = imp_llm::Result<StreamEvent>> + Send>> {
            Box::pin(futures::stream::empty())
        }

        async fn resolve_auth(&self, _auth: &AuthStore) -> imp_llm::Result<ApiKey> {
            Ok("test-key".to_string())
        }

        fn id(&self) -> &str {
            "mock"
        }

        fn models(&self) -> &[ModelMeta] {
            &[]
        }
    }

    fn test_model() -> Model {
        Model {
            meta: ModelMeta {
                id: "test-model".to_string(),
                provider: "mock".to_string(),
                name: "Test Model".to_string(),
                context_window: 200_000,
                max_output_tokens: 4096,
                pricing: ModelPricing {
                    input_per_mtok: 3.0,
                    output_per_mtok: 15.0,
                    cache_read_per_mtok: 0.3,
                    cache_write_per_mtok: 3.75,
                },
                capabilities: Capabilities {
                    reasoning: false,
                    images: false,
                    tool_use: true,
                },
            },
            provider: Arc::new(MockProvider),
        }
    }

    #[test]
    fn builder_applies_config_max_tokens() {
        let config = Config {
            max_tokens: Some(2048),
            ..Default::default()
        };

        let (agent, _handle) =
            AgentBuilder::new(config, PathBuf::from("/tmp"), test_model(), "key".into())
                .build()
                .unwrap();

        assert_eq!(agent.max_tokens, Some(2048));
    }

    #[test]
    fn builder_applies_context_config_thresholds() {
        let mut config = Config::default();
        config.context.observation_mask_threshold = 0.5;
        config.context.mask_window = 7;

        let (agent, _handle) =
            AgentBuilder::new(config, PathBuf::from("/tmp"), test_model(), "key".into())
                .build()
                .unwrap();

        assert!((agent.context_config.observation_mask_threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(agent.context_config.mask_window, 7);
    }

    #[test]
    fn builder_default_config_uses_standard_thresholds() {
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!((agent.context_config.observation_mask_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(agent.context_config.mask_window, 10);
    }

    #[test]
    fn builder_system_prompt_override_skips_discovery() {
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .system_prompt("custom system prompt".into())
        .build()
        .unwrap();

        assert_eq!(agent.system_prompt, "custom system prompt");
    }

    #[test]
    fn builder_api_key_wired() {
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "my-api-key".into(),
        )
        .build()
        .unwrap();

        assert_eq!(agent.api_key, "my-api-key");
    }

    #[test]
    fn builder_extra_tools_registered() {
        use crate::tools::{Tool, ToolContext, ToolOutput};

        struct DummyTool;

        #[async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str {
                "dummy"
            }
            fn label(&self) -> &str {
                "Dummy"
            }
            fn description(&self) -> &str {
                "A dummy tool for testing"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            fn is_readonly(&self) -> bool {
                true
            }
            async fn execute(
                &self,
                _call_id: &str,
                _params: serde_json::Value,
                _ctx: ToolContext,
            ) -> crate::error::Result<ToolOutput> {
                Ok(ToolOutput::text("ok"))
            }
        }

        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .extra_tools(|tools| tools.register(Arc::new(DummyTool)))
        .build()
        .unwrap();

        assert!(agent.tools.get("dummy").is_some());
    }

    #[test]
    fn builder_registers_canonical_tools_and_compat_aliases() {
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(agent.tools.get("bash").is_some());
        assert!(agent.tools.get("shell").is_none());
        assert!(agent.tools.get("sh").is_none());
        assert!(agent.tools.get("ask_agent").is_none());
        assert!(agent.tools.get("imp").is_none());
        assert!(agent.tools.get("spawn").is_none());
        assert!(agent.tools.get("edit").is_some());
        assert!(agent.tools.get("multi_edit").is_none());
        assert!(agent.tools.get("memory").is_none());
        assert!(agent.tools.get("recall").is_none());
        assert!(agent.tools.get("session_search").is_none());
        assert!(agent.tools.get("git").is_some());
        assert!(agent.tools.get("worktree").is_none());

        let mut definition_names: Vec<_> = agent
            .tools
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        definition_names.sort();

        assert!(definition_names.contains(&"bash".to_string()));
        assert!(!definition_names.contains(&"ask_agent".to_string()));
        assert!(!definition_names.contains(&"spawn".to_string()));
        assert!(definition_names.contains(&"edit".to_string()));
        assert!(!definition_names.contains(&"imp".to_string()));
        assert!(!definition_names.contains(&"multi_edit".to_string()));
        assert!(!definition_names.contains(&"recall".to_string()));
        assert!(!definition_names.contains(&"session_search".to_string()));
        assert!(!definition_names.contains(&"memory".to_string()));
        assert!(!definition_names.contains(&"worktree".to_string()));
    }

    #[test]
    fn builder_filters_tower_memory_outside_tower_projects() {
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", temp.path());

        let imp_dir = temp.path().join("imp");
        std::fs::create_dir_all(&imp_dir).unwrap();
        std::fs::write(
            imp_dir.join("memory.md"),
            "Project lives at /Users/asher/tower and uses root workflow.",
        )
        .unwrap();
        std::fs::write(
            imp_dir.join("user.md"),
            "User prefers root workflow in /tower for Tower work.",
        )
        .unwrap();

        let mut config = Config::default();
        config.learning.enabled = true;

        let (agent, _handle) = AgentBuilder::new(
            config,
            PathBuf::from("/tmp/not-tower/project"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(!agent.system_prompt.contains("/Users/asher/tower"));
        assert!(!agent.system_prompt.contains("/tower for Tower work"));

        if let Some(prev) = prev {
            std::env::set_var("XDG_CONFIG_HOME", prev);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn builder_keeps_tower_memory_inside_tower_projects() {
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", temp.path());

        let imp_dir = temp.path().join("imp");
        std::fs::create_dir_all(&imp_dir).unwrap();
        std::fs::write(
            imp_dir.join("memory.md"),
            "Project lives at /Users/asher/tower and uses root workflow.",
        )
        .unwrap();

        let mut config = Config::default();
        config.learning.enabled = true;

        let (agent, _handle) = AgentBuilder::new(
            config,
            PathBuf::from("/Users/asher/tower/imp"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(agent.system_prompt.contains("/Users/asher/tower"));

        if let Some(prev) = prev {
            std::env::set_var("XDG_CONFIG_HOME", prev);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    #[cfg(any())]
    fn builder_injects_workflow_facts_into_system_prompt_when_available() {
        let temp = tempfile::TempDir::new().unwrap();
        let workflow_dir = temp.path().join(".imp/workflows");
        std::fs::create_dir(&workflow_dir).unwrap();

        let workflow_config = workflow_core::config::Config {
            project: "test".to_string(),
            ..Default::default()
        };
        workflow_config.save(&workflow_dir).unwrap();

        let mut working = workflow_core::unit::Unit::new("1", "Implement auth flow");
        working.status = workflow_core::unit::Status::InProgress;
        working.paths = vec!["src/auth.rs".to_string()];
        working.requires = vec!["AuthProvider".to_string()];
        let working_slug = workflow_core::util::title_to_slug(&working.title);
        working
            .to_file(workflow_dir.join(format!("1-{}.md", working_slug)))
            .unwrap();

        let mut fact = workflow_core::unit::Unit::new("2", "Auth uses RS256 signing");
        fact.kind = workflow_core::unit::UnitType::Fact;
        fact.unit_type = "fact".to_string();
        fact.paths = vec!["src/auth.rs".to_string()];
        fact.produces = vec!["AuthProvider".to_string()];
        fact.last_verified = Some(chrono::Utc::now() - chrono::Duration::hours(2));
        let fact_slug = workflow_core::util::title_to_slug(&fact.title);
        fact.to_file(workflow_dir.join(format!("2-{}.md", fact_slug)))
            .unwrap();

        std::fs::create_dir(temp.path().join("src")).unwrap();
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            temp.path().join("src"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(agent.system_prompt.contains("Project facts:"));
        assert!(agent.system_prompt.contains("Auth uses RS256 signing"));
        assert!(agent.system_prompt.contains("verified"));
        assert!(agent.system_prompt.contains("Project memory status:"));
        assert!(agent.system_prompt.contains("Working on:"));
        assert!(agent.system_prompt.contains("[1] Implement auth flow"));
    }

    #[test]
    #[cfg(any())]
    fn builder_injects_project_memory_status_into_system_prompt_when_available() {
        let temp = tempfile::TempDir::new().unwrap();
        let workflow_dir = temp.path().join(".imp/workflows");
        std::fs::create_dir(&workflow_dir).unwrap();

        let workflow_config = workflow_core::config::Config {
            project: "test".to_string(),
            ..Default::default()
        };
        workflow_config.save(&workflow_dir).unwrap();

        let mut working = workflow_core::unit::Unit::new("1", "Implement auth flow");
        working.status = workflow_core::unit::Status::InProgress;
        working.claimed_by = Some("imp".to_string());
        let working_slug = workflow_core::util::title_to_slug(&working.title);
        working
            .to_file(workflow_dir.join(format!("1-{}.md", working_slug)))
            .unwrap();

        let mut recent = workflow_core::unit::Unit::new("3", "Recently closed cleanup");
        recent.status = workflow_core::unit::Status::Closed;
        recent.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(2));
        let recent_slug = workflow_core::util::title_to_slug(&recent.title);
        let archive_dir = workflow_dir.join("archive").join("2026").join("05");
        std::fs::create_dir_all(&archive_dir).unwrap();
        recent
            .to_file(archive_dir.join(format!("3-{}.md", recent_slug)))
            .unwrap();

        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            temp.path().join("src"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(agent.system_prompt.contains("Project memory status:"));
        assert!(agent.system_prompt.contains("Working on:"));
        assert!(agent.system_prompt.contains("[1] Implement auth flow"));
        assert!(agent.system_prompt.contains("Recent work:"));
        assert!(agent.system_prompt.contains("[3] Recently closed cleanup"));
    }

    #[test]
    fn builder_task_fact_override_does_not_add_project_memory_status() {
        let facts = vec![Fact {
            text: "Auth uses RS256 signing".into(),
            verified_ago: "2h ago".into(),
        }];

        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .facts(facts)
        .build()
        .unwrap();

        assert!(agent.system_prompt.contains("Project facts:"));
        assert!(!agent.system_prompt.contains("Project memory status:"));
    }

    #[test]
    fn builder_applies_coder_role_tools_prompt_and_workflow_contract() {
        let registry = Config::default().role_registry().unwrap();
        let role = registry.resolve("coder").unwrap();
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .role(role)
        .build()
        .unwrap();

        assert!(agent.tools.get("edit").is_some());
        assert!(agent.tools.get("write").is_some());
        assert!(agent
            .system_prompt
            .contains("Make the smallest coherent code change"));
        assert!(agent
            .workflow_contract()
            .closeout_criteria
            .criteria
            .iter()
            .any(|criterion| criterion.contains("diff-summary")));
        assert!(agent
            .workflow_contract()
            .tool_permissions
            .allowed_tools
            .contains("edit"));
        assert!(agent.workflow_contract().required_verification.is_empty());
    }

    #[test]
    fn builder_applies_verifier_role_readonly_tools_and_contract_hints() {
        let registry = Config::default().role_registry().unwrap();
        let mut role = registry.resolve("verifier").unwrap();
        role.verification.suggested_commands = vec!["cargo test -p imp-core role_registry".into()];
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .role(role)
        .build()
        .unwrap();

        assert!(agent.role.as_ref().unwrap().readonly);
        assert!(agent.tools.get("read").is_some());
        assert!(agent.tools.get("bash").is_some());
        assert!(agent.tools.get("edit").is_none());
        assert!(agent.tools.get("write").is_none());
        assert!(agent
            .system_prompt
            .contains("Run only declared or clearly relevant verification commands"));
        assert_eq!(agent.workflow_contract().required_verification.len(), 1);
        assert!(agent
            .workflow_contract()
            .closeout_criteria
            .criteria
            .iter()
            .any(|criterion| criterion.contains("test-output")));
    }

    #[test]
    fn builder_preserves_default_behavior_without_role() {
        let (agent, _handle) = AgentBuilder::new(
            Config::default(),
            PathBuf::from("/tmp"),
            test_model(),
            "key".into(),
        )
        .build()
        .unwrap();

        assert!(agent.role.is_none());
        assert!(agent.tools.get("edit").is_some());
        assert!(agent.tools.get("write").is_some());
        assert!(agent
            .workflow_contract()
            .closeout_criteria
            .criteria
            .is_empty());
    }

    #[test]
    fn builder_hooks_loaded_from_config() {
        use crate::hooks::HookDef;

        let mut config = Config::default();
        config.hooks.push(HookDef {
            event: "after_file_write".into(),
            match_pattern: Some("*.rs".into()),
            action: "shell".into(),
            command: Some("echo hook fired".into()),
            blocking: false,
            threshold: None,
        });

        let (agent, _handle) =
            AgentBuilder::new(config, PathBuf::from("/tmp"), test_model(), "key".into())
                .build()
                .unwrap();

        // Hooks were loaded — the runner should have one registered hook
        assert_eq!(agent.hooks.len(), 1);
    }
}
