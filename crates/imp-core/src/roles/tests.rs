use super::*;
use std::collections::HashMap;

fn builtin_map() -> HashMap<&'static str, RoleDef> {
    builtin_roles().into_iter().collect()
}

#[test]
fn builtin_roles_include_practical_workflow_roles_and_compatibility_aliases() {
    let roles = builtin_map();
    for name in [
        "planner",
        "coder",
        "verifier",
        "reviewer",
        "researcher",
        "integrator",
        "worker",
        "explorer",
    ] {
        assert!(roles.contains_key(name), "missing builtin role {name}");
    }
}

#[test]
fn builtin_roles_have_expected_readonly_and_autonomy_behavior() {
    let roles = builtin_map();
    let planner = Role::from_def("planner", roles.get("planner").unwrap());
    let coder = Role::from_def("coder", roles.get("coder").unwrap());
    let verifier = Role::from_def("verifier", roles.get("verifier").unwrap());
    let reviewer = Role::from_def("reviewer", roles.get("reviewer").unwrap());
    let researcher = Role::from_def("researcher", roles.get("researcher").unwrap());
    let integrator = Role::from_def("integrator", roles.get("integrator").unwrap());

    assert!(planner.readonly);
    assert!(planner.autonomy.can_create_workflows);
    assert!(!planner.autonomy.can_modify_files);
    assert!(!coder.readonly);
    assert!(coder.autonomy.can_modify_files);
    assert!(coder.verification.required);
    assert!(verifier.readonly);
    assert!(verifier.autonomy.can_run_commands);
    assert!(!verifier.autonomy.can_modify_files);
    assert!(reviewer.readonly);
    assert!(researcher.readonly);
    assert!(!integrator.readonly);
    assert!(integrator.child_workflow.can_coordinate_children);
}

#[test]
fn builtin_roles_expose_tool_constraints_and_evidence() {
    let roles = builtin_map();
    let reviewer = Role::from_def("reviewer", roles.get("reviewer").unwrap());
    let coder = Role::from_def("coder", roles.get("coder").unwrap());
    let verifier = Role::from_def("verifier", roles.get("verifier").unwrap());

    assert!(matches!(reviewer.tool_policy, RoleToolPolicy::Only(_)));
    assert!(matches!(coder.tool_policy, RoleToolPolicy::Only(_)));
    assert!(coder
        .required_evidence
        .iter()
        .any(|item| item.kind == "diff-summary"));
    assert!(verifier
        .required_evidence
        .iter()
        .any(|item| item.kind == "test-output"));
    assert!(verifier
        .model_routing
        .capability_hints
        .contains(&"test-debugging".into()));
}

#[test]
fn role_registry_merges_overrides_and_resolves_aliases() {
    let mut override_def = builtin_map().remove("coder").unwrap();
    override_def.instructions = Some("Custom coder instruction".into());
    override_def.instruction_set.clear();
    let registry = RoleRegistry::from_overrides(vec![("coder".into(), override_def)]).unwrap();
    let coder = registry.resolve("coder").unwrap();
    let worker = registry.resolve("worker").unwrap();
    assert_eq!(coder.instruction_set, vec!["Custom coder instruction"]);
    assert_eq!(worker.instruction_set, vec!["Custom coder instruction"]);
    assert!(registry.role_names().any(|name| name == "planner"));
}

#[test]
fn role_registry_rejects_invalid_role_name_and_unknown_tool() {
    let invalid_name = RoleRegistry::from_overrides(vec![("BadRole".into(), RoleDef::default())]);
    assert!(matches!(
        invalid_name,
        Err(RoleRegistryError::InvalidRoleName(_))
    ));

    let invalid_tool = RoleRegistry::from_overrides(vec![(
        "custom".into(),
        RoleDef {
            instructions: Some("Custom role".into()),
            tools: Some(vec!["definitely-not-a-tool".into()]),
            ..RoleDef::default()
        },
    )]);
    assert!(matches!(
        invalid_tool,
        Err(RoleRegistryError::InvalidTool { .. })
    ));
}

#[test]
fn role_registry_rejects_readonly_write_tool_but_allows_verifier_bash() {
    let readonly_write = RoleRegistry::from_overrides(vec![(
        "readonly-writer".into(),
        RoleDef {
            readonly: true,
            instructions: Some("No writes".into()),
            tools: Some(vec!["read".into(), "write".into()]),
            ..RoleDef::default()
        },
    )]);
    assert!(matches!(
        readonly_write,
        Err(RoleRegistryError::ReadonlyWriteTool { .. })
    ));

    let registry = RoleRegistry::from_overrides(Vec::<(String, RoleDef)>::new()).unwrap();
    let verifier = registry.resolve("verifier").unwrap();
    assert!(verifier.readonly);
    assert!(verifier.autonomy.can_run_commands);
    assert!(matches!(verifier.tool_policy, RoleToolPolicy::Only(_)));
}

#[test]
fn role_registry_def_rejects_unknown_alias_target() {
    let registry = RoleRegistryDef {
        aliases: BTreeMap::from([("old".into(), "missing".into())]),
        ..RoleRegistryDef::default()
    }
    .into_registry();
    assert!(matches!(
        registry,
        Err(RoleRegistryError::UnknownAliasTarget { .. })
    ));
}

#[test]
fn model_routing_prefers_explicit_model_alias() {
    let registry = RoleRegistry::from_overrides(vec![(
        "custom".into(),
        RoleDef {
            instructions: Some("Custom routing".into()),
            model_routing: Some(RoleModelRouting {
                preferred_model: Some("haiku".into()),
                model_classes: vec![RoleModelClass::HighReasoning],
                ..RoleModelRouting::default()
            }),
            ..RoleDef::default()
        },
    )])
    .unwrap();
    let models = ModelRegistry::with_builtins();
    let model = registry.resolve_model_for_role("custom", &models).unwrap();
    assert_eq!(model.id, "claude-haiku-4-5-20251001");
}

#[test]
fn model_routing_resolves_default_role_hints_through_model_registry() {
    let registry = RoleRegistry::from_overrides(Vec::<(String, RoleDef)>::new()).unwrap();
    let models = ModelRegistry::with_builtins();
    let planner = registry.resolve_model_for_role("planner", &models).unwrap();
    let coder = registry.resolve_model_for_role("coder", &models).unwrap();
    let researcher = registry
        .resolve_model_for_role("researcher", &models)
        .unwrap();

    assert!(planner.capabilities.reasoning);
    assert!(coder.capabilities.tool_use);
    assert!(researcher.context_window >= 128_000);
    assert_ne!(planner.id, "");
    assert_ne!(coder.id, "");
}

#[test]
fn output_schema_metadata_roundtrips() {
    let schema = RoleOutputSchema {
        name: "verification-result".into(),
        description: "Verifier output shape".into(),
        json_schema_ref: Some("schemas/verification-result.schema.json".into()),
        required_sections: vec!["status".into(), "commands".into()],
        output_contract: Some("Return pass, fail, or blocked with command evidence.".into()),
        example: Some("status: failed\ncommands:\n- cargo test".into()),
    };
    let json = serde_json::to_string(&schema).unwrap();
    let decoded: RoleOutputSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, schema);
}

#[test]
fn role_registry_serde_accepts_existing_role_def_shape() {
    let json = r#"{
        "model": "haiku",
        "thinking": "off",
        "tools": ["read", "scan"],
        "readonly": true,
        "instructions": "Explore only."
    }"#;

    let def: RoleDef = serde_json::from_str(json).unwrap();
    assert_eq!(def.model.as_deref(), Some("haiku"));
    assert_eq!(def.tools, Some(vec!["read".into(), "scan".into()]));
    assert!(def.readonly);
    assert_eq!(def.instructions.as_deref(), Some("Explore only."));
}

#[test]
fn role_registry_serde_accepts_workflow_metadata() {
    let json = r#"{
        "purpose": "Check acceptance criteria",
        "prompt_template": "verifier.md",
        "instruction_set": ["Run only declared verification commands."],
        "tool_policy": { "only": ["read", "bash"] },
        "autonomy": {
            "can_run_commands": true,
            "stop_on_verification_failure": true,
            "max_consecutive_tool_calls": 8
        },
        "required_evidence": [
            { "kind": "test-output", "required": true, "description": "Verifier command output" }
        ],
        "verification": {
            "required": true,
            "suggested_commands": ["cargo test -p imp-core role_registry"],
            "accepts_manual_evidence": false
        },
        "model_routing": {
            "preferred_model": "sonnet",
            "fallback_models": ["haiku"],
            "thinking": "high",
            "latency_preference": "balanced",
            "cost_preference": "quality",
            "model_classes": ["high-reasoning", "review"],
            "capability_hints": ["test-debugging", "structured-output"]
        },
        "output_schema": {
            "name": "verification-result",
            "required_sections": ["status", "commands"]
        },
        "child_workflow": {
            "eligible": true,
            "max_children": 1,
            "allowed_child_roles": ["coder"]
        }
    }"#;

    let def: RoleDef = serde_json::from_str(json).unwrap();
    assert_eq!(def.purpose.as_deref(), Some("Check acceptance criteria"));
    assert_eq!(def.required_evidence[0].kind, "test-output");
    assert_eq!(
        def.model_routing.as_ref().unwrap().model_classes,
        vec![RoleModelClass::HighReasoning, RoleModelClass::Review]
    );
    assert_eq!(
        def.model_routing.as_ref().unwrap().capability_hints,
        vec!["test-debugging", "structured-output"]
    );
    assert!(def.child_workflow.as_ref().unwrap().eligible);
}

#[test]
fn role_registry_resolves_existing_readonly_role_def() {
    let def = RoleDef {
        model: Some("haiku".into()),
        thinking: Some(ThinkingLevel::Off),
        readonly: true,
        instructions: Some("Read only".into()),
        ..RoleDef::default()
    };

    let role = Role::from_def("explorer", &def);
    assert_eq!(role.name, "explorer");
    assert!(role.readonly);
    assert!(matches!(role.tool_set, ToolSet::Only(_)));
    assert!(!role.autonomy.can_modify_files);
    assert!(!role.autonomy.can_run_commands);
    assert_eq!(role.model_routing.preferred_model.as_deref(), Some("haiku"));
    assert_eq!(role.instruction_set, vec!["Read only"]);
}

#[test]
fn role_registry_resolves_workflow_metadata() {
    let def = RoleDef {
        purpose: Some("Implement focused changes".into()),
        prompt_template: Some("coder.md".into()),
        readonly: false,
        autonomy: Some(RoleAutonomy {
            can_modify_files: true,
            can_run_commands: true,
            max_consecutive_tool_calls: Some(12),
            ..RoleAutonomy::default()
        }),
        required_evidence: vec![EvidenceRequirement {
            kind: "diff-summary".into(),
            required: true,
            description: "Changed files and rationale".into(),
        }],
        verification: Some(RoleVerification {
            required: true,
            suggested_commands: vec!["cargo test".into()],
            ..RoleVerification::default()
        }),
        output_schema: Some(RoleOutputSchema {
            name: "implementation-summary".into(),
            required_sections: vec!["changed".into(), "verified".into()],
            ..RoleOutputSchema::default()
        }),
        ..RoleDef::default()
    };

    let role = Role::from_def("coder", &def);
    assert_eq!(role.purpose.as_deref(), Some("Implement focused changes"));
    assert_eq!(role.prompt_template.as_deref(), Some("coder.md"));
    assert!(role.autonomy.can_modify_files);
    assert_eq!(role.required_evidence[0].kind, "diff-summary");
    assert!(role.verification.required);
    assert_eq!(role.output_schema.unwrap().name, "implementation-summary");
}
