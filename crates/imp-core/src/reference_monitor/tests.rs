use super::*;

#[test]
fn tool_metadata_classifies_native_tools() {
    let read = ToolMetadata::for_tool_name("read", true);
    assert_eq!(read.action_kind, ToolActionKind::Read);
    assert!(read.readonly);
    assert!(!read.workspace_write);

    let write = ToolMetadata::for_tool_name("write", false);
    assert_eq!(write.action_kind, ToolActionKind::Write);
    assert!(write.workspace_write);
    assert!(!write.readonly);

    let edit = ToolMetadata::for_tool_name("edit", false);
    assert_eq!(edit.action_kind, ToolActionKind::Edit);
    assert!(edit.workspace_write);

    let bash = ToolMetadata::for_tool_name("bash", false);
    assert_eq!(bash.action_kind, ToolActionKind::Execute);
    assert!(bash.external_side_effect);
    assert!(bash.default_requires_approval);

    let git = ToolMetadata::for_tool_name("git", false);
    assert_eq!(git.action_kind, ToolActionKind::Git);
    assert!(git.external_side_effect);
    assert!(git.workspace_write);

    let workflow = ToolMetadata::for_tool_name("workflow", false);
    assert_eq!(workflow.action_kind, ToolActionKind::Workflow);
    assert!(workflow.external_side_effect);

    let web = ToolMetadata::for_tool_name("web", true);
    assert_eq!(web.action_kind, ToolActionKind::Network);
    assert!(web.network);
}

#[test]
fn tool_metadata_classifies_extension_placeholder() {
    let metadata = ToolMetadata::for_tool_name("lua:deploy", false);
    assert_eq!(metadata.action_kind, ToolActionKind::Extension);
    assert!(metadata.extension);
    assert_eq!(metadata.extension_id.as_deref(), Some("lua:deploy"));
    assert!(metadata.external_side_effect);
}

#[test]
fn tool_metadata_extracts_resource_scope_from_args() {
    let read = ToolMetadata::for_tool_name("read", true);
    let scope = read.resource_scope_for_args(
        Some(std::path::Path::new("/repo")),
        &serde_json::json!({ "path": "src/lib.rs" }),
    );
    assert_eq!(
        scope,
        ResourceScope::File {
            path: std::path::PathBuf::from("/repo/src/lib.rs")
        }
    );

    let bash = ToolMetadata::for_tool_name("bash", false);
    assert_eq!(
        bash.resource_scope_for_args(None, &serde_json::json!({ "command": "cargo test" })),
        ResourceScope::Command {
            program: "cargo".into()
        }
    );

    let workflow = ToolMetadata::for_tool_name("workflow", false);
    assert_eq!(
        workflow.resource_scope_for_args(None, &serde_json::json!({ "action": "close" })),
        ResourceScope::Workflow {
            action: Some("close".into())
        }
    );
}

#[test]
fn extension_secret_capability_is_denied_before_config_policy() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("secret_ext", ToolActionKind::Extension);
    context.metadata.extension = true;
    context.metadata.secrets = true;
    context.policy.secrets = PolicyAction::Allow;

    let decision = monitor.check_tool_action(&context, &RunPolicy::default());
    assert!(matches!(
        decision,
        ToolPolicyDecision::Deny { reason } if reason.code == "extension_secret_denied"
    ));
}

#[test]
fn extension_network_capability_uses_config_policy() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("net_ext", ToolActionKind::Extension);
    context.metadata.extension = true;
    context.metadata.network = true;

    let decision = monitor.check_tool_action(&context, &RunPolicy::default());
    assert!(matches!(
        decision,
        ToolPolicyDecision::Deny { reason } if reason.code == "policy_extension_network_denied"
    ));

    context.policy.extension_network = PolicyAction::Allow;
    let decision = monitor.check_tool_action(&context, &RunPolicy::default());
    assert!(matches!(decision, ToolPolicyDecision::Allow { .. }));
}

#[test]
fn config_policy_allows_extension_readonly_capability() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("readonly_ext", ToolActionKind::Read);
    context.metadata.extension = true;
    context.metadata.readonly = true;
    context.metadata.external_side_effect = false;
    context.metadata.workspace_write = false;

    let decision = monitor.check_tool_action(&context, &RunPolicy::default());
    assert!(matches!(decision, ToolPolicyDecision::Allow { .. }));
}

#[test]
fn reference_monitor_matches_run_policy_tool_allow_and_deny() {
    let monitor = ReferenceMonitor;
    let allowed_policy = RunPolicy::new().allow_tool("read");
    let denied_policy = RunPolicy::new().deny_tool("bash");

    let read = ToolPolicyContext::new("read", ToolActionKind::Read);
    assert!(monitor
        .check_tool_action(&read, &allowed_policy)
        .is_allowed());

    let bash = ToolPolicyContext::new("bash", ToolActionKind::Execute);
    let run_policy_decision = denied_policy.check_tool("bash");
    let monitor_decision = monitor.check_tool_action(&bash, &denied_policy);
    match (run_policy_decision, monitor_decision) {
        (RunToolDecision::Denied(expected), ToolPolicyDecision::Deny { reason }) => {
            assert_eq!(reason.source, PolicySource::RunPolicy);
            assert_eq!(reason.code, "run_policy_tool_denied");
            assert_eq!(reason.message, expected);
        }
        other => panic!("unexpected decisions: {other:?}"),
    }
}

#[test]
fn reference_monitor_applies_agent_mode_before_run_policy() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("write", ToolActionKind::Write);
    context.mode = AgentMode::Reviewer;
    let decision = monitor.check_tool_action(&context, &RunPolicy::new().allow_tool("write"));
    match decision {
        ToolPolicyDecision::Deny { ref reason } => {
            assert_eq!(reason.source, PolicySource::AgentMode);
            assert_eq!(reason.code, "agent_mode_tool_denied");
        }
        other => panic!("expected deny, got {other:?}"),
    }
}

#[test]
fn reference_monitor_matches_run_policy_write_path() {
    let monitor = ReferenceMonitor;
    let policy = RunPolicy::new().allow_tool("write").allow_write("src/**");
    let cwd = std::path::PathBuf::from("/repo");
    let mut context = ToolPolicyContext::new("write", ToolActionKind::Write);
    context.cwd = Some(cwd.clone());
    context.metadata = ToolMetadata::for_tool_name("write", false);
    context.resource_scope = ResourceScope::File {
        path: std::path::PathBuf::from("/repo/README.md"),
    };

    let write_policy_decision =
        policy.check_write_path(&cwd, std::path::Path::new("/repo/README.md"));
    let monitor_decision = monitor.check_tool_action(&context, &policy);
    match (write_policy_decision, monitor_decision) {
        (WritePolicyDecision::Denied(expected), ToolPolicyDecision::Deny { reason }) => {
            assert_eq!(reason.source, PolicySource::RunPolicy);
            assert_eq!(reason.code, "run_policy_write_path_denied");
            assert_eq!(reason.message, expected);
        }
        other => panic!("unexpected decisions: {other:?}"),
    }

    context.resource_scope = ResourceScope::File {
        path: std::path::PathBuf::from("/repo/src/lib.rs"),
    };
    assert!(monitor.check_tool_action(&context, &policy).is_allowed());
}

#[test]
fn reference_monitor_evaluate_returns_trace_record() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("bash", ToolActionKind::Execute);
    context.run_id = Some("run_1".into());
    let record = monitor.evaluate(&context, &RunPolicy::new().deny_tool("bash"));
    assert_eq!(record.run_id.as_deref(), Some("run_1"));
    assert_eq!(record.tool_name, "bash");
    assert!(matches!(record.decision, ToolPolicyDecision::Deny { .. }));
}

#[test]
fn policy_trace_records_cover_scattered_policy_outcomes() {
    let monitor = ReferenceMonitor;
    let context = ToolPolicyContext::new("bash", ToolActionKind::Execute);

    let hook = crate::hooks::HookResult {
        block: true,
        reason: Some("blocked by hook".into()),
        modified_content: None,
    };
    assert_policy_record(
        monitor.hook_blocked_record(&context, &hook),
        PolicySource::Hook,
        "hook_blocked",
    );

    assert_policy_record(
        monitor.bash_equivalent_record(&context, "use workflow tool"),
        PolicySource::BashEquivalent,
        "policy_blocked",
    );

    assert_policy_record(
        monitor.repeated_call_record(&context, true, "loop detected"),
        PolicySource::RepeatedCall,
        "repeated_tool_call_blocked",
    );

    assert_policy_record(
        monitor.repeated_call_record(&context, false, "possible loop"),
        PolicySource::RepeatedCall,
        "repeated_tool_call_warned",
    );

    assert_policy_record(
        monitor.validation_error_record(&context, "bad args"),
        PolicySource::Schema,
        "validation_error",
    );

    assert_policy_record(
        monitor.guardrail_record(
            &context,
            crate::guardrails::GuardrailLevel::Enforce,
            true,
            "guardrail failed",
        ),
        PolicySource::Guardrail,
        "guardrail_enforced",
    );
}

fn assert_policy_record(record: PolicyTraceRecord, source: PolicySource, code: &str) {
    match record.decision {
        ToolPolicyDecision::Allow { reasons } => {
            assert!(reasons
                .iter()
                .any(|reason| reason.source == source && reason.code == code));
        }
        ToolPolicyDecision::Deny { reason }
        | ToolPolicyDecision::AskUser { reason }
        | ToolPolicyDecision::DryRunOnly { reason }
        | ToolPolicyDecision::SandboxOnly { reason }
        | ToolPolicyDecision::RequireVerification { reason } => {
            assert_eq!(reason.source, source);
            assert_eq!(reason.code, code);
        }
    }
}

#[test]
fn reference_monitor_context_defaults_preserve_absent_contract_behavior() {
    let context = ToolPolicyContext::new("read", ToolActionKind::Read);
    assert_eq!(context.policy, PolicyConfig::default());
    assert_eq!(context.workflow_type, WorkflowType::AdHoc);
    assert_eq!(context.risk_level, RiskLevel::Unknown);
    assert_eq!(context.workspace_scope, WorkspaceScope::CurrentDirectory);
    assert_eq!(context.trust_scope, TrustScopeContext::default());
    assert_eq!(
        context.trust_labels,
        Vec::<String>::new(),
        "labels are only populated when a workflow contract is explicitly threaded"
    );
    assert!(ReferenceMonitor
        .check_tool_action(&context, &RunPolicy::new())
        .is_allowed());
}

#[test]
fn reference_monitor_context_accepts_workflow_contract_without_policy_mode() {
    let mut contract = WorkflowContract::implicit("local work");
    contract.id = Some("wf-config-policy".into());
    contract.workflow_type = WorkflowType::CodeChange;
    contract.risk_level = RiskLevel::High;
    contract.trust_scope.allow_external_context = false;
    contract.trust_scope.allow_durable_memory_writes = false;

    let context =
        ToolPolicyContext::new("bash", ToolActionKind::Execute).with_workflow_contract(&contract);
    assert_eq!(context.workflow_id.as_deref(), Some("wf-config-policy"));
    assert_eq!(context.workflow_type, WorkflowType::CodeChange);
    assert_eq!(context.risk_level, RiskLevel::High);
    assert_eq!(context.workspace_scope, contract.workspace_scope);
    assert!(!context.trust_scope.allow_external_context);
    assert!(context
        .trust_labels
        .contains(&"external-context-blocked".to_string()));
    assert!(ReferenceMonitor
        .check_tool_action(&context, &RunPolicy::new())
        .is_allowed());
}

#[test]
fn policy_trace_record_includes_trust_scope_and_labels() {
    let mut contract = WorkflowContract::implicit("trusted review");
    contract.trust_scope.low_trust_requires_review = false;
    let context =
        ToolPolicyContext::new("read", ToolActionKind::Read).with_workflow_contract(&contract);
    let record = PolicyTraceRecord::from_context(&context, ToolPolicyDecision::allow());
    assert!(!record.trust_scope.low_trust_requires_review);
    assert!(record
        .trust_labels
        .contains(&"low-trust-review-not-required".to_string()));

    let trace = record.to_trace_event("run_1");
    assert_eq!(trace.kind, "policy.checked");
    assert_eq!(
        trace.payload["trust_scope"]["low_trust_requires_review"],
        false
    );
    assert!(trace.payload["trust_labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label == "low-trust-review-not-required"));
}

#[test]
fn non_allow_decisions_are_serializable_policy_records() {
    let monitor = ReferenceMonitor;
    let context = ToolPolicyContext::new("bash", ToolActionKind::Execute);

    let cases = [
        (
            monitor.ask_user_record(&context, "needs approval"),
            "ask_user",
            "ask_user_required",
            "unsupported_decision",
        ),
        (
            monitor.dry_run_only_record(&context, "dry run first"),
            "dry_run_only",
            "dry_run_required",
            "unsupported_decision",
        ),
        (
            monitor.sandbox_only_record(&context, "sandbox first"),
            "sandbox_only",
            "sandbox_required",
            "unsupported_decision",
        ),
        (
            monitor.require_verification_record(&context, "verify after"),
            "require_verification",
            "require_verification",
            "unsupported_decision",
        ),
    ];

    for (record, decision_name, reason_code, detail_key) in cases {
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["decision"]["decision"], decision_name);
        assert_eq!(json["decision"]["reason"]["code"], reason_code);
        assert!(json["details"].get(detail_key).is_some());
        let trace = record.to_trace_event("run_1");
        assert_eq!(trace.kind, "policy.checked");
        assert_eq!(trace.payload["decision"]["decision"], decision_name);
    }
}

#[test]
fn dangerous_grant_records_fail_closed_above_config_policy() {
    let monitor = ReferenceMonitor;
    let context = ToolPolicyContext::new("bash", ToolActionKind::Execute);
    let rails = [
        (
            DangerousRail::SecretExfiltration,
            "dangerous_secret_exfiltration",
        ),
        (DangerousRail::PrivateKeyRead, "dangerous_private_key_read"),
        (
            DangerousRail::OutsideWorkspaceDestructiveWrite,
            "dangerous_outside_workspace_destructive_write",
        ),
        (DangerousRail::ForcePush, "dangerous_force_push"),
        (
            DangerousRail::GlobalGitConfigMutation,
            "dangerous_global_git_config_mutation",
        ),
        (
            DangerousRail::ProductionDeploy,
            "dangerous_production_deploy",
        ),
        (
            DangerousRail::CloudResourceDeletion,
            "dangerous_cloud_resource_deletion",
        ),
        (
            DangerousRail::AuditLogDisable,
            "dangerous_audit_log_disable",
        ),
    ];

    for (rail, code) in rails {
        let record = monitor.dangerous_grant_required_record(&context, rail);
        match record.decision {
            ToolPolicyDecision::Deny { ref reason } => {
                assert_eq!(reason.source, PolicySource::DangerousGrant);
                assert_eq!(reason.code, code);
                assert!(reason.message.contains("dangerous grant"));
            }
            other => panic!("dangerous rail must deny, got {other:?}"),
        }
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(
            json["details"]["dangerous_rail"],
            serde_json::to_value(rail).unwrap()
        );
    }
}

#[test]
fn config_policy_maps_representative_tool_classes() {
    let monitor = ReferenceMonitor;
    let policy = RunPolicy::new();

    let read = test_context("read", ToolActionKind::Read);
    assert!(monitor.check_tool_action(&read, &policy).is_allowed());

    let mut write = test_context("write", ToolActionKind::Write);
    write.policy.allow_side_effects = false;
    assert_reason_code(
        monitor.check_tool_action(&write, &policy),
        "policy_side_effect_denied",
    );

    let local_write = test_context("write", ToolActionKind::Write);
    assert!(monitor
        .check_tool_action(&local_write, &policy)
        .is_allowed());

    let mut network = test_context("web", ToolActionKind::Network);
    network.metadata.network = true;
    network.resource_scope = ResourceScope::Network {
        host: Some("example.com".into()),
    };
    network.policy.network = PolicyAction::Ask;
    assert_reason_code(
        monitor.check_tool_action(&network, &policy),
        "policy_network_requires_approval",
    );

    let mut browser = test_context("browser", ToolActionKind::Browser);
    browser.metadata.requires_approval = false;
    assert_reason_code(
        monitor.check_tool_action(&browser, &policy),
        "policy_browser_input_requires_approval",
    );
    browser.metadata.requires_approval = true;
    assert_reason_code(
        monitor.check_tool_action(&browser, &policy),
        "policy_browser_input_requires_approval",
    );
    browser.policy.browser_input = PolicyAction::Allow;
    assert!(monitor.check_tool_action(&browser, &policy).is_allowed());
    browser.policy.browser_input = PolicyAction::Deny;
    assert_reason_code(
        monitor.check_tool_action(&browser, &policy),
        "policy_browser_input_denied",
    );

    let mut secret = test_context("secret", ToolActionKind::Secret);
    secret.metadata.secrets = true;
    assert_reason_code(
        monitor.check_tool_action(&secret, &policy),
        "policy_secret_denied",
    );

    let mut ci_like = test_context("bash", ToolActionKind::Execute);
    ci_like.metadata.default_requires_approval = true;
    ci_like.policy.deny_approval_required = true;
    assert_reason_code(
        monitor.check_tool_action(&ci_like, &policy),
        "policy_approval_required_denied",
    );
}

#[test]
fn config_policy_handles_outside_workspace_writes() {
    let monitor = ReferenceMonitor;
    let policy = RunPolicy::new();

    let mut outside = test_context("write", ToolActionKind::Write);
    outside.cwd = Some(std::path::PathBuf::from("/repo"));
    outside.resource_scope = ResourceScope::File {
        path: std::path::PathBuf::from("/tmp/file"),
    };
    assert_reason_code(
        monitor.check_tool_action(&outside, &policy),
        "policy_write_denied",
    );

    outside.policy.outside_workspace_writes = PolicyAction::Ask;
    assert_reason_code(
        monitor.check_tool_action(&outside, &policy),
        "policy_write_requires_approval",
    );
}

#[test]
fn config_policy_allows_native_writes_to_registered_worktrees() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let worktree = temp.path().join("worktree");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "test\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "initial"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "feature/policy-test",
            worktree.to_str().unwrap(),
        ],
    );

    let mut context = test_context("write", ToolActionKind::Write);
    context.cwd = Some(repo);
    context.resource_scope = ResourceScope::File {
        path: worktree.join("new/file.rs"),
    };
    assert!(ReferenceMonitor
        .check_tool_action(&context, &RunPolicy::new())
        .is_allowed());
}

#[test]
fn config_policy_preserves_existing_run_policy_precedence() {
    let monitor = ReferenceMonitor;
    let mut safe = test_context("bash", ToolActionKind::Execute);
    safe.metadata.default_requires_approval = true;
    safe.policy.deny_approval_required = true;
    assert!(matches!(
        monitor.check_tool_action(&safe, &RunPolicy::new()),
        ToolPolicyDecision::Deny { .. }
    ));

    assert_reason_code(
        monitor.check_tool_action(&safe, &RunPolicy::new().deny_tool("bash")),
        "run_policy_tool_denied",
    );
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap()
        .success());
}

fn test_context(name: &str, kind: ToolActionKind) -> ToolPolicyContext {
    let mut context = ToolPolicyContext::new(name, kind);
    context.metadata = ToolMetadata::for_tool_name(
        name,
        matches!(kind, ToolActionKind::Read | ToolActionKind::Search),
    );
    context.cwd = Some(std::path::PathBuf::from("/repo"));
    if context.metadata.workspace_write
        || matches!(kind, ToolActionKind::Write | ToolActionKind::Edit)
    {
        context.resource_scope = ResourceScope::File {
            path: std::path::PathBuf::from("/repo/src/lib.rs"),
        };
    }
    context
}

fn assert_reason_code(decision: ToolPolicyDecision, code: &str) {
    match decision {
        ToolPolicyDecision::Deny { reason }
        | ToolPolicyDecision::AskUser { reason }
        | ToolPolicyDecision::DryRunOnly { reason }
        | ToolPolicyDecision::SandboxOnly { reason }
        | ToolPolicyDecision::RequireVerification { reason } => assert_eq!(reason.code, code),
        other => panic!("expected non-allow decision {code}, got {other:?}"),
    }
}

#[test]
fn reference_monitor_denies_low_trust_high_risk_escalation() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("bash", ToolActionKind::Execute)
        .with_supporting_provenance(
            Provenance::external_web("https://example.com/instructions")
                .with_risk(crate::trust::RiskLabel::ContainsInstructions),
        );
    context.metadata = ToolMetadata::for_tool_name("bash", false);

    let decision = monitor.check_tool_action(&context, &RunPolicy::new());
    match decision {
        ToolPolicyDecision::Deny { reason } => {
            assert_eq!(reason.source, PolicySource::TrustLabel);
            assert_eq!(reason.code, "low_trust_escalation_denied");
            assert!(reason.message.contains("https://example.com/instructions"));
        }
        other => panic!("expected trust denial, got {other:?}"),
    }
}

#[test]
fn reference_monitor_asks_user_for_low_trust_network_escalation() {
    let monitor = ReferenceMonitor;
    let mut context = ToolPolicyContext::new("web", ToolActionKind::Network)
        .with_supporting_provenance(Provenance::external_web("https://example.com"));
    context.metadata = ToolMetadata::for_tool_name("web", false);
    context.resource_scope = ResourceScope::Network {
        host: Some("api.example.com".into()),
    };

    let decision = monitor.check_tool_action(&context, &RunPolicy::new());
    match decision {
        ToolPolicyDecision::AskUser { reason } => {
            assert_eq!(reason.source, PolicySource::TrustLabel);
            assert_eq!(reason.code, "low_trust_escalation_denied");
        }
        other => panic!("expected trust approval request, got {other:?}"),
    }
}

#[test]
fn reference_monitor_allows_trusted_support_and_low_risk_low_trust_context() {
    let monitor = ReferenceMonitor;
    let trusted = ToolPolicyContext::new("bash", ToolActionKind::Execute)
        .with_supporting_provenance(Provenance::user_instruction());
    assert!(monitor
        .check_tool_action(&trusted, &RunPolicy::new())
        .is_allowed());

    let low_risk = ToolPolicyContext::new("read", ToolActionKind::Read)
        .with_supporting_provenance(Provenance::external_web("https://example.com"));
    assert!(monitor
        .check_tool_action(&low_risk, &RunPolicy::new())
        .is_allowed());
}

#[test]
fn reference_monitor_types_default_to_safe_unknown_context() {
    let context = ToolPolicyContext::new("mystery", ToolActionKind::Unknown);
    assert_eq!(context.tool_name, "mystery");
    assert_eq!(context.mode, AgentMode::Full);
    assert_eq!(context.policy, PolicyConfig::default());
    assert_eq!(context.resource_scope, ResourceScope::None);
    assert_eq!(
        context.metadata,
        ToolMetadata::new("mystery", ToolActionKind::Unknown)
    );
    assert!(ToolPolicyDecision::default().is_allowed());
}

#[test]
fn reference_monitor_types_serialize_decision_variants() {
    let reason = PolicyReason::new(PolicySource::RunPolicy, "deny_tool", "tool denied");
    let decision = ToolPolicyDecision::Deny { reason };
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["decision"], "deny");
    assert_eq!(json["reason"]["source"], "run_policy");
    assert_eq!(json["reason"]["code"], "deny_tool");
}

#[test]
fn reference_monitor_types_trace_record_carries_context() {
    let mut context = ToolPolicyContext::new("bash", ToolActionKind::Execute);
    context.run_id = Some("run_1".into());
    context.workflow_id = Some("394.5".into());
    context.turn = Some(2);
    context.tool_call_id = Some("call_1".into());
    context.resource_scope = ResourceScope::Command {
        program: "cargo".into(),
    };

    let record = PolicyTraceRecord::from_context(&context, ToolPolicyDecision::allow());
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["run_id"], "run_1");
    assert_eq!(json["workflow_id"], "394.5");
    assert_eq!(json["tool_name"], "bash");
    assert_eq!(json["action_kind"], "execute");
    assert_eq!(json["decision"]["decision"], "allow");
}
