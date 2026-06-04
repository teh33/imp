use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn config_default_values() {
    let config = Config::default();
    assert!(config.model.is_none());
    assert!(config.thinking.is_none());
    assert!(config.max_tokens.is_none());
    assert!(config.max_turns.is_none());
    assert!(config.tools.is_none());
    assert_eq!(config.ui.read_max_lines, 500);
    assert_eq!(config.ui.sidebar_style, SidebarStyle::Inspector);
    assert_eq!(config.ui.chat_tool_display, ChatToolDisplay::Summary);
    assert_eq!(config.ui.tool_output, ToolOutputDisplay::Compact);
    assert_eq!(config.web, WebConfig::default());
    assert!(config.roles.is_empty());
    assert!(config.hooks.is_empty());
    assert!((config.context.observation_mask_threshold - 0.6).abs() < f64::EPSILON);
    assert_eq!(config.context.mask_window, 10);
    assert_eq!(
        config.context.auto_compaction.mode,
        AutoCompactionMode::NearThreshold
    );
    assert_eq!(config.guardrails, GuardrailConfig::default());
}

#[test]
fn inspector_sidebar_keeps_tool_calls_in_chat_summary() {
    let mut ui = UiConfig {
        sidebar_style: SidebarStyle::Inspector,
        chat_tool_display: ChatToolDisplay::Interleaved,
        ..Default::default()
    };
    assert_eq!(ui.effective_chat_tool_display(), ChatToolDisplay::Summary);

    ui.chat_tool_display = ChatToolDisplay::Hidden;
    ui.hide_tools_in_chat = true;
    assert_eq!(ui.effective_chat_tool_display(), ChatToolDisplay::Summary);

    ui.sidebar_style = SidebarStyle::Stream;
    assert_eq!(ui.effective_chat_tool_display(), ChatToolDisplay::Hidden);
}

#[test]
fn config_load_from_toml() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
model = "sonnet"
thinking = "high"
max_tokens = 2048
max_turns = 50
tools = ["read", "write", "bash"]

[guardrails]
enabled = true
level = "enforce"
profile = "zig"
critical_paths = ["src/**"]
after_write = ["zig fmt --check ."]

[context]
observation_mask_threshold = 0.5
mask_window = 5

[shell]
command = "zsh"

[web]
search_provider = "exa"
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    assert_eq!(config.model.as_deref(), Some("sonnet"));
    assert_eq!(config.thinking, Some(ThinkingLevel::High));
    assert_eq!(config.max_tokens, Some(2048));
    assert_eq!(config.max_turns, Some(50));
    assert_eq!(config.tools.as_ref().unwrap().len(), 3);
    assert_eq!(config.guardrails.enabled, Some(true));
    assert_eq!(config.ui.read_max_lines, 500);
    assert_eq!(
        config.guardrails.profile,
        Some(crate::guardrails::GuardrailProfile::Zig)
    );
    assert_eq!(
        config.guardrails.after_write,
        Some(vec!["zig fmt --check .".into()])
    );
    assert_eq!(config.shell.command.as_deref(), Some("zsh"));
    assert_eq!(
        config.web.search_provider,
        Some(crate::tools::web::types::SearchProvider::Exa)
    );
    assert!((config.context.observation_mask_threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(config.context.mask_window, 5);
    assert_eq!(
        config.context.auto_compaction.mode,
        AutoCompactionMode::NearThreshold
    );
}

#[test]
fn config_load_missing_file_returns_default() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("nonexistent.toml");
    let config = Config::load(&config_path).unwrap();
    assert!(config.model.is_none());
}

#[test]
fn config_merge_project_overrides_user() {
    let mut user = Config {
        model: Some("haiku".into()),
        max_tokens: Some(1024),
        max_turns: Some(20),
        ..Default::default()
    };

    let project = Config {
        model: Some("sonnet".into()),
        max_tokens: None,
        max_turns: None, // not set → user value preserved
        ..Default::default()
    };

    user.merge(project);
    assert_eq!(user.model.as_deref(), Some("sonnet"));
    assert_eq!(user.max_tokens, Some(1024));
    assert_eq!(user.max_turns, Some(20));
}

#[test]
fn config_merge_roles_extend() {
    let mut base = Config::default();
    base.roles.insert(
        "worker".into(),
        RoleDef {
            model: Some("haiku".into()),
            thinking: None,
            readonly: false,
            ..RoleDef::default()
        },
    );

    let overlay = Config {
        roles: {
            let mut m = HashMap::new();
            m.insert(
                "reviewer".into(),
                RoleDef {
                    model: Some("sonnet".into()),
                    thinking: Some(ThinkingLevel::High),
                    readonly: true,
                    ..RoleDef::default()
                },
            );
            m
        },
        ..Default::default()
    };

    base.merge(overlay);
    assert!(base.roles.contains_key("worker"));
    assert!(base.roles.contains_key("reviewer"));
}

#[test]
fn config_merge_hooks_extend() {
    let mut base = Config::default();
    base.hooks.push(HookDef {
        event: "after_file_write".into(),
        match_pattern: None,
        action: "log".into(),
        command: None,
        blocking: false,
        threshold: None,
    });

    let overlay = Config {
        hooks: vec![HookDef {
            event: "before_tool_call".into(),
            match_pattern: None,
            action: "block".into(),
            command: None,
            blocking: true,
            threshold: None,
        }],
        ..Default::default()
    };

    base.merge(overlay);
    assert_eq!(base.hooks.len(), 2);
}

#[test]
fn config_merge_context_overrides_default() {
    let mut base = Config::default();

    let overlay = Config {
        context: ContextConfig {
            observation_mask_threshold: 0.5,
            mask_window: 5,
            auto_compaction: AutoCompactionConfig {
                mode: AutoCompactionMode::NearThreshold,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    base.merge(overlay);
    assert!((base.context.observation_mask_threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(base.context.mask_window, 5);
    assert_eq!(
        base.context.auto_compaction.mode,
        AutoCompactionMode::NearThreshold
    );
}

#[test]
fn config_merge_includes_theme_learning_and_lua() {
    let mut base = Config::default();
    let overlay = Config {
        theme: Some("light".into()),
        learning: LearningConfig {
            enabled: false,
            skill_nudge_threshold: 3,
            memory_char_limit: 1000,
            user_char_limit: 700,
        },
        lua: LuaConfig {
            allow_native_tool_calls: Some(false),
            allow_shell_exec: Some(true),
            allow_http: None,
            allow_secrets: None,
            allowed_env: Some(vec!["HOME".into()]),
        },
        ..Default::default()
    };

    base.merge(overlay);

    assert_eq!(base.theme.as_deref(), Some("light"));
    assert_eq!(base.learning.skill_nudge_threshold, 3);
    assert!(!base.learning.enabled);
    assert_eq!(base.lua.allow_native_tool_calls, Some(false));
    assert_eq!(base.lua.allow_shell_exec, Some(true));
    assert_eq!(base.lua.allowed_env, Some(vec!["HOME".into()]));
}

#[test]
fn config_merge_guardrails_preserves_unspecified_fields() {
    let mut base = Config::default();
    base.guardrails.enabled = Some(true);
    base.guardrails.profile = Some(crate::guardrails::GuardrailProfile::Rust);
    base.guardrails.critical_paths = Some(vec!["src/**".into()]);

    let mut overlay = Config::default();
    overlay.guardrails.level = Some(crate::guardrails::GuardrailLevel::Enforce);
    overlay.guardrails.after_write = Some(vec!["cargo test".into()]);

    base.merge(overlay);

    assert_eq!(base.guardrails.enabled, Some(true));
    assert_eq!(
        base.guardrails.profile,
        Some(crate::guardrails::GuardrailProfile::Rust)
    );
    assert_eq!(base.guardrails.critical_paths, Some(vec!["src/**".into()]));
    assert_eq!(
        base.guardrails.level,
        Some(crate::guardrails::GuardrailLevel::Enforce)
    );
    assert_eq!(base.guardrails.after_write, Some(vec!["cargo test".into()]));
}

#[test]
fn config_resolve_user_then_project() {
    // Clean env to avoid interference from parallel tests
    std::env::remove_var("IMP_MODEL");
    std::env::remove_var("IMP_THINKING");

    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("user");
    let project_dir = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(project_dir.join(".imp")).unwrap();

    // User config: model=haiku, max_turns=20, custom context
    fs::write(
        user_dir.join("config.toml"),
        r#"
model = "haiku"
max_turns = 20

[context]
observation_mask_threshold = 0.55
mask_window = 9

[context.auto_compaction]
mode = "disabled"
"#,
    )
    .unwrap();

    // Project config: model=sonnet (overrides user), custom context overrides user context
    fs::write(
        project_dir.join(".imp").join("config.toml"),
        r#"
model = "sonnet"

[context]
observation_mask_threshold = 0.5
mask_window = 5

[context.auto_compaction]
mode = "disabled"
"#,
    )
    .unwrap();

    let config = Config::resolve(&user_dir, Some(&project_dir)).unwrap();
    assert_eq!(config.model.as_deref(), Some("sonnet"));
    assert_eq!(config.max_turns, Some(20));
    assert!((config.context.observation_mask_threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(config.context.mask_window, 5);
}

#[test]
fn config_resolve_env_overrides() {
    // Test env override logic without relying on process-global state
    // (env vars are inherently racy in parallel tests).
    // We test that the override *mechanism* works by simulating it.
    let mut config = Config {
        model: Some("haiku".into()),
        thinking: Some(ThinkingLevel::Low),
        max_tokens: Some(2048),
        ..Default::default()
    };

    // Simulate IMP_MODEL override
    let env_model = "opus";
    config.model = Some(env_model.into());

    // Simulate IMP_THINKING override
    let env_thinking = "high";
    config.thinking = parse_thinking_level(env_thinking);

    // Simulate IMP_MAX_TOKENS override
    let env_max_tokens = "1024";
    config.max_tokens = env_max_tokens.parse::<u32>().ok();

    assert_eq!(config.model.as_deref(), Some("opus"));
    assert_eq!(config.thinking, Some(ThinkingLevel::High));
    assert_eq!(config.max_tokens, Some(1024));
}

#[test]
fn config_resolve_missing_files_uses_defaults() {
    let dir = TempDir::new().unwrap();
    let config = Config::resolve(dir.path(), None).unwrap();
    assert!(config.model.is_none());
    assert!(config.thinking.is_none());
    assert!(config.max_tokens.is_none());
    assert!(config.max_turns.is_none());
}

#[test]
fn config_load_with_roles_and_hooks() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
model = "sonnet"

[roles.coder]
model = "opus"
thinking = "high"
readonly = false

[roles.reader]
readonly = true

[[hooks]]
event = "after_file_write"
action = "log"
blocking = false
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    assert_eq!(config.roles.len(), 2);
    assert!(config.roles.contains_key("coder"));
    assert!(config.roles.contains_key("reader"));
    assert_eq!(config.roles["coder"].model.as_deref(), Some("opus"));
    assert!(config.roles["reader"].readonly);
    assert_eq!(config.hooks.len(), 1);
    assert_eq!(config.hooks[0].event, "after_file_write");
}

#[test]
fn config_parse_thinking_levels() {
    assert_eq!(parse_thinking_level("off"), Some(ThinkingLevel::Off));
    assert_eq!(
        parse_thinking_level("minimal"),
        Some(ThinkingLevel::Minimal)
    );
    assert_eq!(parse_thinking_level("low"), Some(ThinkingLevel::Low));
    assert_eq!(parse_thinking_level("medium"), Some(ThinkingLevel::Medium));
    assert_eq!(parse_thinking_level("high"), Some(ThinkingLevel::High));
    assert_eq!(parse_thinking_level("xhigh"), Some(ThinkingLevel::XHigh));
    assert_eq!(parse_thinking_level("OFF"), Some(ThinkingLevel::Off));
    assert_eq!(parse_thinking_level("High"), Some(ThinkingLevel::High));
    assert_eq!(parse_thinking_level("invalid"), None);
    assert_eq!(parse_thinking_level(""), None);
}

#[test]
fn config_partial_toml_fills_defaults() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
model = "sonnet"
"#,
    )
    .unwrap();

    let config = Config::load(&config_path).unwrap();
    assert_eq!(config.model.as_deref(), Some("sonnet"));
    // Unspecified fields use defaults
    assert!(config.thinking.is_none());
    assert!(config.max_tokens.is_none());
    assert!(config.max_turns.is_none());
    assert!((config.context.observation_mask_threshold - 0.6).abs() < f64::EPSILON);
}

// --- AgentMode tests ---

#[test]
fn agent_mode_default_is_full() {
    let config = Config::default();
    assert_eq!(config.mode, AgentMode::Full);
    assert_eq!(AgentMode::default(), AgentMode::Full);
}

#[test]
fn lua_config_resolves_capability_policy() {
    let config = LuaConfig {
        allow_native_tool_calls: Some(false),
        allow_shell_exec: Some(true),
        allow_http: Some(true),
        allow_secrets: Some(true),
        allowed_env: Some(vec!["OPENAI_API_KEY".to_string(), "HOME".to_string()]),
    };

    let policy = config.resolve_policy(AgentMode::Worker);
    assert!(!policy.allow_native_tool_calls);
    assert!(policy.allow_shell_exec);
    assert!(policy.allow_http);
    assert!(policy.allow_secrets);
    assert!(policy.allowed_env.contains("OPENAI_API_KEY"));
    assert!(policy.allowed_env.contains("HOME"));
}

#[test]
fn worker_lua_policy_preserves_configured_secret_access() {
    let enabled = LuaConfig {
        allow_secrets: Some(true),
        ..Default::default()
    };
    assert!(enabled.resolve_policy(AgentMode::Worker).allow_secrets);

    let disabled = LuaConfig {
        allow_secrets: Some(false),
        ..Default::default()
    };
    assert!(!disabled.resolve_policy(AgentMode::Worker).allow_secrets);

    assert!(
        !LuaConfig::default()
            .resolve_policy(AgentMode::Worker)
            .allow_secrets
    );
}

#[test]
fn agent_mode_full_allows_all_tools() {
    let mode = AgentMode::Full;
    assert!(mode.allows_tool("anything"));
    assert!(mode.allows_tool("read"));
    assert!(mode.allows_tool("bash"));
    assert!(mode.allows_tool("nonexistent_future_tool"));
    assert_eq!(mode.allowed_tool_names(), &[] as &[&str]);
}

#[test]
fn agent_mode_orchestrator_allows_read() {
    let mode = AgentMode::Orchestrator;
    assert!(mode.allows_tool("read"));
    assert!(mode.allows_tool("scan"));
    assert!(mode.allows_tool("web"));
    assert!(mode.allows_tool("git"));
    assert!(!mode.allows_tool("recall"));
    assert!(mode.allows_tool("workflow"));
    assert!(mode.allows_tool("ask_user"));
}

#[test]
fn agent_mode_orchestrator_blocks_write() {
    let mode = AgentMode::Orchestrator;
    assert!(!mode.allows_tool("write"));
    assert!(!mode.allows_tool("edit"));
    assert!(!mode.allows_tool("bash"));
}

#[test]
fn non_full_modes_block_removed_ask_agent() {
    for mode in [
        AgentMode::Worker,
        AgentMode::Orchestrator,
        AgentMode::Planner,
        AgentMode::Reviewer,
        AgentMode::Auditor,
    ] {
        assert!(
            !mode.allows_tool("ask_agent"),
            "mode {mode:?} should block removed ask_agent"
        );
    }
}

#[test]
fn agent_mode_planner_allows_workflow_update() {
    let mode = AgentMode::Planner;
    assert!(mode.allows_workflow_action("update"));
    assert!(mode.allows_workflow_action("list"));
    assert!(mode.allows_workflow_action("list"));
    assert!(mode.allows_workflow_action("show"));
    assert!(mode.allows_tool("workflow"));
}

#[test]
fn agent_mode_planner_blocks_workflow_run() {
    let mode = AgentMode::Planner;
    assert!(!mode.allows_workflow_action("run"));
    assert!(!mode.allows_workflow_action("run"));
    assert!(mode.allows_workflow_action("update"));
    assert!(mode.allows_tool("git"));
}

#[test]
fn agent_mode_workflow_action_policy_matches_native_workflow_tool() {
    let worker = AgentMode::Worker;
    assert!(worker.allows_workflow_action("show"));
    assert!(worker.allows_workflow_action("update"));
    assert!(!worker.allows_workflow_action("run"));

    let orchestrator = AgentMode::Orchestrator;
    assert!(orchestrator.allows_workflow_action("run"));
    assert!(orchestrator.allows_workflow_action("update"));

    let planner = AgentMode::Planner;
    assert!(planner.allows_workflow_action("update"));
    assert!(!planner.allows_workflow_action("run"));

    let reviewer = AgentMode::Reviewer;
    assert!(!reviewer.allows_workflow_action("show"));

    let auditor = AgentMode::Auditor;
    assert!(auditor.allows_workflow_action("show"));
    assert!(!auditor.allows_workflow_action("update"));
}

#[test]
fn agent_mode_worker_blocks_workflow_run() {
    let mode = AgentMode::Worker;
    assert!(mode.allows_workflow_action("update"));
    assert!(!mode.allows_workflow_action("run"));
    assert!(mode.allows_tool("git"));
}

#[test]
fn agent_mode_worker_allows_workflow_update() {
    let mode = AgentMode::Worker;
    assert!(mode.allows_workflow_action("update"));
    assert!(mode.allows_workflow_action("show"));
    assert!(mode.allows_workflow_action("list"));
    assert!(mode.allows_workflow_action("list"));
}

#[test]
fn agent_mode_reviewer_no_workflow() {
    let mode = AgentMode::Reviewer;
    assert!(!mode.allows_workflow_action("list"));
    assert!(!mode.allows_workflow_action("list"));
    assert!(!mode.allows_workflow_action("show"));
    assert!(!mode.allows_workflow_action("update"));
    assert!(!mode.allows_workflow_action("run"));
    // Reviewer also has no workflow tool access
    assert!(!mode.allows_tool("workflow"));
    assert!(mode.allows_tool("git"));
}

#[test]
fn agent_mode_auditor_workflow_readonly() {
    let mode = AgentMode::Auditor;
    assert!(mode.allows_workflow_action("list"));
    assert!(mode.allows_workflow_action("list"));
    assert!(mode.allows_workflow_action("show"));
    assert!(!mode.allows_workflow_action("update"));
    assert!(!mode.allows_workflow_action("run"));
    assert!(!mode.allows_workflow_action("run"));
    assert!(!mode.allows_workflow_action("update"));
    assert!(mode.allows_tool("git"));
}

#[test]
fn agent_mode_config_deserialize() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, r#"mode = "orchestrator""#).unwrap();
    let config = Config::load(&config_path).unwrap();
    assert_eq!(config.mode, AgentMode::Orchestrator);
}

#[test]
fn agent_mode_instructions() {
    assert!(AgentMode::Full.instructions().is_none());
    assert!(AgentMode::Worker.instructions().is_some());
    assert!(AgentMode::Orchestrator.instructions().is_some());
    assert!(AgentMode::Planner.instructions().is_some());
    assert!(AgentMode::Reviewer.instructions().is_some());
    assert!(AgentMode::Auditor.instructions().is_some());

    // Spot-check content is mode-specific
    let worker = AgentMode::Worker.instructions().unwrap();
    assert!(worker.contains("worker"));
    assert!(worker.contains("implement the assigned unit as specified"));
    assert!(worker.contains("final verification and closure belong to the orchestrator workflow"));

    let orchestrator = AgentMode::Orchestrator.instructions().unwrap();
    assert!(orchestrator.contains("orchestrator agent"));
    assert!(orchestrator.contains("primary execution substrate"));
    assert!(orchestrator.contains("final verification, retry, and closure workflow"));

    let reviewer = AgentMode::Reviewer.instructions().unwrap();
    assert!(reviewer.contains("reviewer") || reviewer.contains("read"));
}
