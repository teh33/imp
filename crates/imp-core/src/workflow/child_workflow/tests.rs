use super::*;
use crate::agent::{RunFinalStatus, StopReason};
use crate::config::Config;

use crate::workflow::{
    AutonomyMode, ToolPermissionSet, WorkflowStepAction, WorkflowStepIsolation,
    WorkflowStepOutputContract, WorkflowType,
};

fn registry() -> RoleRegistry {
    Config::default().role_registry().unwrap()
}

#[test]
fn workflow_subagent_spawn_builds_bounded_input_and_started_event() {
    let action = WorkflowStepAction {
        kind: crate::workflow::WorkflowStepActionKind::Agent,
        role: Some("verifier".into()),
        worker: None,
        objective: "Verify workflow dispatch".into(),
        instructions: vec!["Run the focused workflow tests.".into()],
        write_scope: vec![PathBuf::from(".imp/workflows/demo/results.md")],
        completion: crate::workflow::WorkflowStepActionCompletion {
            checks: vec!["tests_passed".into()],
            artifacts: vec![PathBuf::from(".imp/workflows/demo/results.md")],
        },
        output: WorkflowStepOutputContract {
            required_sections: vec!["Decision".into(), "Evidence".into()],
        },
        review: None,
        isolation: WorkflowStepIsolation::default(),
    };

    let spawn = workflow_subagent_spawn("demo", "verify", &action);

    assert!(spawn
        .input
        .output_contract
        .as_deref()
        .unwrap_or_default()
        .contains("Required output sections: Decision, Evidence"));
    assert_eq!(spawn.input.parent_run_id.as_str(), "workflow-demo");
    assert_eq!(spawn.input.child_run_id.as_str(), "workflow-demo-verify");
    assert_eq!(spawn.input.role, SubagentRole::Verifier);
    assert_eq!(spawn.input.merge_policy, SubagentMergePolicy::Verify);
    assert_eq!(
        spawn.input.resource_limits.writable_paths,
        action.write_scope
    );
    assert!(spawn
        .input
        .output_contract
        .as_deref()
        .unwrap_or_default()
        .contains("Required checks: tests_passed"));
    assert_eq!(
        spawn.started_event,
        SubagentEvent::Started {
            child_run_id: SubagentRunId::new("workflow-demo-verify"),
            role: SubagentRole::Verifier,
            objective: "Verify workflow dispatch".into(),
        }
    );
}

#[test]
fn workflow_output_contract_sections_are_included_in_subagent_input() {
    let action = WorkflowStepAction {
        kind: crate::workflow::WorkflowStepActionKind::Agent,
        role: Some("reviewer".into()),
        worker: None,
        objective: "Review workflow output shape".into(),
        instructions: Vec::new(),
        write_scope: vec![PathBuf::from(".imp/workflows/demo/results.md")],
        completion: crate::workflow::WorkflowStepActionCompletion::default(),
        output: WorkflowStepOutputContract {
            required_sections: vec!["Decision".into(), "Evidence".into(), "Concerns".into()],
        },
        review: None,
        isolation: WorkflowStepIsolation::default(),
    };

    let input = workflow_subagent_input("demo", "review", &action);
    let contract = input.output_contract.as_deref().expect("output contract");

    assert!(contract.contains("Required output sections: Decision, Evidence, Concerns"));
    assert_eq!(input.merge_policy, SubagentMergePolicy::Review);
}

#[test]
fn workflow_discoveries_artifact_is_included_in_subagent_input() {
    let action = WorkflowStepAction {
        kind: crate::workflow::WorkflowStepActionKind::Agent,
        role: Some("coder".into()),
        worker: None,
        objective: "Use workflow discoveries".into(),
        instructions: Vec::new(),
        write_scope: vec![PathBuf::from("src/lib.rs")],
        completion: crate::workflow::WorkflowStepActionCompletion::default(),
        output: WorkflowStepOutputContract::default(),
        review: None,
        isolation: WorkflowStepIsolation::default(),
    };

    let input = workflow_subagent_input("demo", "build", &action);

    assert!(input.context.artifacts.iter().any(|artifact| {
        artifact.name == "workflow discoveries"
            && artifact.path.as_deref()
                == Some(std::path::Path::new(
                    ".imp/workflows/demo/artifacts/discoveries.md",
                ))
    }));
}

#[test]
fn workflow_subagent_completion_maps_final_status_to_outcome_event() {
    let action = WorkflowStepAction {
        kind: crate::workflow::WorkflowStepActionKind::Agent,
        role: Some("coder".into()),
        worker: None,
        objective: "Implement workflow dispatch".into(),
        instructions: Vec::new(),
        write_scope: vec![PathBuf::from("src/lib.rs")],
        completion: crate::workflow::WorkflowStepActionCompletion::default(),
        output: WorkflowStepOutputContract::default(),
        review: None,
        isolation: WorkflowStepIsolation::default(),
    };
    let input = workflow_subagent_input("demo", "build", &action);

    let completion = workflow_subagent_completion(
        &input,
        Some(&RunFinalStatus::DoneWithConcerns {
            reason: StopReason::NoProgress,
            concerns: vec!["manual review recommended".into()],
        }),
    );

    assert_eq!(completion.status, SubagentStatus::Incomplete);
    assert!(completion.summary.contains("manual review recommended"));
    let SubagentEvent::Completed { outcome } = completion.event else {
        panic!("expected completed event");
    };
    assert_eq!(outcome.child_run_id.as_str(), "workflow-demo-build");
    assert_eq!(outcome.role, SubagentRole::Implementer);
    assert_eq!(outcome.status, SubagentStatus::Incomplete);
    assert_eq!(outcome.files_changed, vec![PathBuf::from("src/lib.rs")]);
}

#[test]
fn workflow_failure_summary_artifact_is_added_for_failed_subagent() {
    let action = WorkflowStepAction {
        kind: crate::workflow::WorkflowStepActionKind::Agent,
        role: Some("coder".into()),
        worker: None,
        objective: "Fail usefully".into(),
        instructions: Vec::new(),
        write_scope: vec![PathBuf::from("src/lib.rs")],
        completion: crate::workflow::WorkflowStepActionCompletion::default(),
        output: WorkflowStepOutputContract::default(),
        review: None,
        isolation: WorkflowStepIsolation::default(),
    };
    let input = workflow_subagent_input("demo", "build", &action);

    let completion = workflow_subagent_completion(
        &input,
        Some(&RunFinalStatus::Failed {
            message: "tests failed".into(),
        }),
    );

    let SubagentEvent::Completed { outcome } = completion.event else {
        panic!("expected completed event");
    };
    assert_eq!(outcome.status, SubagentStatus::Failed);
    assert!(outcome.evidence.iter().any(|artifact| {
        artifact.name == "failure summary"
            && artifact.path.as_deref().is_some_and(|path| {
                path.ends_with("artifacts/failures/workflow-demo-build-attempt-1.md")
            })
    }));
    let failure_summary = outcome
        .diagnostics
        .first()
        .expect("structured failure summary diagnostic");
    assert!(failure_summary.contains("# Subagent failure summary"));
    assert!(failure_summary.contains("## Failure\n\ntests failed"));
    assert!(failure_summary.contains("## Touched files\n\n- src/lib.rs"));
    assert!(failure_summary.contains("## Next attempt guidance"));
    assert_eq!(outcome.blockers, vec![failure_summary.clone()]);
}

#[test]
fn child_workflow_final_status_maps_to_child_status() {
    assert_eq!(
        child_status_from_final_status(Some(&RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        })),
        ChildWorkflowStatus::Done
    );
    assert_eq!(
        child_status_from_final_status(Some(&RunFinalStatus::DoneWithConcerns {
            reason: StopReason::NoProgress,
            concerns: vec!["verification unavailable".into()],
        })),
        ChildWorkflowStatus::DoneWithConcerns
    );
    assert_eq!(
        child_status_from_final_status(Some(&RunFinalStatus::Blocked {
            reason: StopReason::ExecutionBlocked,
            message: "blocked".into(),
        })),
        ChildWorkflowStatus::Blocked
    );
    assert_eq!(
        child_status_from_final_status(Some(&RunFinalStatus::Cancelled)),
        ChildWorkflowStatus::Cancelled
    );
    assert_eq!(
        child_status_from_final_status(None),
        ChildWorkflowStatus::Failed
    );
}

#[test]
fn child_contract_inherits_parent_context_and_applies_verifier_role() {
    let registry = registry();
    let role = registry.resolve("verifier").unwrap();
    let parent = WorkflowContract {
        id: Some("parent-workflow".into()),
        objective: "Fix parser empty input".into(),
        workflow_type: WorkflowType::CodeChange,
        autonomy_mode: AutonomyMode::AllowAllLocal,
        tool_permissions: ToolPermissionSet::default()
            .allow("read")
            .allow("edit")
            .allow("bash"),
        workflow_unit_ref: Some("394.13".into()),
        ..WorkflowContract::default()
    };

    let contract = create_child_workflow_contract(
        &parent,
        &role,
        "child-verify-1",
        "Verify parser fix",
        "Run parser tests",
    );

    assert_eq!(contract.id.as_deref(), Some("child-verify-1"));
    assert_eq!(
        contract.parent_workflow_ref.as_deref(),
        Some("parent-workflow")
    );
    assert_eq!(contract.workflow_unit_ref.as_deref(), Some("394.13"));
    assert_eq!(contract.role.as_deref(), Some("verifier"));
    assert_eq!(contract.workflow_type, WorkflowType::Verification);
    assert_eq!(contract.autonomy_mode, AutonomyMode::Safe);
    assert!(contract
        .objective
        .contains("Parent objective: Fix parser empty input"));
    assert!(contract.objective.contains("Child task: Run parser tests"));
    assert!(contract.tool_permissions.allowed_tools.contains("bash"));
    assert!(!contract.tool_permissions.allowed_tools.contains("edit"));
    assert!(contract
        .closeout_criteria
        .criteria
        .iter()
        .any(|criterion| criterion.contains("test-output")));
}

#[test]
fn child_contract_inherits_parent_verification_and_appends_role_verification() {
    let registry = registry();
    let mut role = registry.resolve("verifier").unwrap();
    role.verification.suggested_commands = vec!["cargo test -p imp-core child_workflow".into()];
    let parent = WorkflowContract {
        id: Some("parent-workflow".into()),
        objective: "Parent".into(),
        required_verification: vec![VerificationRequirement::command("cargo test -p imp-core")],
        ..WorkflowContract::default()
    };

    let contract = create_child_workflow_contract(
        &parent,
        &role,
        "child-verify-2",
        "Verify child",
        "Run focused child test",
    );

    assert_eq!(contract.required_verification.len(), 2);
    assert!(contract
        .required_verification
        .iter()
        .any(|requirement| matches!(
            &requirement.kind,
            crate::workflow::VerificationRequirementKind::Command { command }
                if command == "cargo test -p imp-core"
        )));
    assert!(contract
        .required_verification
        .iter()
        .any(|requirement| matches!(
            &requirement.kind,
            crate::workflow::VerificationRequirementKind::Command { command }
                if command == "cargo test -p imp-core child_workflow"
        )));
}

#[test]
fn child_workflow_run_serde_roundtrip_preserves_metadata() {
    let spec = ChildWorkflowSpec {
        id: ChildWorkflowId::new("parent/children/verifier-1"),
        parent: ParentWorkflowRef {
            workflow_id: Some("parent-workflow".into()),
            run_id: Some("run-parent".into()),
            workflow_unit_ref: Some("394.13".into()),
        },
        parent_contract: WorkflowContract {
            id: Some("parent-workflow".into()),
            objective: "Parent parser workflow".into(),
            ..WorkflowContract::default()
        },
        role: "verifier".into(),
        title: "Verify parser".into(),
        prompt: "Run parser tests".into(),
        contract_ref: Some(".imp/runs/run-parent/children/verifier-1/contract.json".into()),
        contract: WorkflowContract {
            id: Some("child-contract".into()),
            role: Some("verifier".into()),
            ..WorkflowContract::default()
        },
        required: true,
    };
    let mut run = ChildWorkflowRun::new(spec);
    run.evidence_refs.push(ChildEvidenceRef {
        kind: "test-output".into(),
        path: ".imp/runs/run-parent/children/verifier-1/evidence.md".into(),
        summary: Some("parser tests failed".into()),
    });
    run.summary = Some(ChildWorkflowSummary {
        status: ChildWorkflowStatus::DoneWithConcerns,
        summary: "verification completed with a failure".into(),
        findings: vec!["parser_empty_input failed".into()],
        concerns: vec!["needs fix".into()],
    });
    run.transition(ChildWorkflowStatus::Running, Some("started".into()));
    run.transition(
        ChildWorkflowStatus::DoneWithConcerns,
        Some("finished with concerns".into()),
    );

    let json = serde_json::to_string_pretty(&run).unwrap();
    let decoded: ChildWorkflowRun = serde_json::from_str(&json).unwrap();

    assert_eq!(
        decoded.spec.id,
        ChildWorkflowId::new("parent/children/verifier-1")
    );
    assert_eq!(
        decoded.spec.parent.workflow_unit_ref.as_deref(),
        Some("394.13")
    );
    assert_eq!(decoded.spec.contract.role.as_deref(), Some("verifier"));
    assert_eq!(decoded.status, ChildWorkflowStatus::DoneWithConcerns);
    assert_eq!(decoded.evidence_refs[0].kind, "test-output");
    assert_eq!(decoded.lifecycle.len(), 3);
    assert!(decoded.completed_at.is_some());
}

#[test]
fn child_workflow_status_transition_helpers_cover_parent_blocking() {
    assert!(!ChildWorkflowStatus::Running.is_terminal());
    assert!(ChildWorkflowStatus::Done.is_terminal());
    assert!(ChildWorkflowStatus::Failed.blocks_required_parent());
    assert!(ChildWorkflowStatus::Cancelled.blocks_required_parent());
    assert!(!ChildWorkflowStatus::DoneWithConcerns.blocks_required_parent());

    let mut run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    run.transition(ChildWorkflowStatus::Running, Some("started".into()));
    assert_eq!(run.status, ChildWorkflowStatus::Running);
    assert!(run.started_at.is_some());
    run.transition(ChildWorkflowStatus::Done, Some("done".into()));
    assert_eq!(run.status, ChildWorkflowStatus::Done);
    assert!(run.completed_at.is_some());
}

#[test]
fn child_workflow_cancellation_and_stale_metadata_are_recorded() {
    let mut run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    run.request_cancellation("user requested", Some("user".into()));
    assert_eq!(run.status, ChildWorkflowStatus::Cancelling);
    assert_eq!(run.cancellation.as_ref().unwrap().reason, "user requested");
    assert_eq!(
        run.cancellation.as_ref().unwrap().requested_by.as_deref(),
        Some("user")
    );

    run.mark_stale("idle timeout", 300);
    assert_eq!(run.status, ChildWorkflowStatus::Stale);
    assert_eq!(run.stale.as_ref().unwrap().idle_timeout_secs, 300);
    assert_eq!(run.stale.as_ref().unwrap().reason, "idle timeout");
}

#[test]
fn stale_detection_marks_idle_child_as_stale() {
    let mut run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    run.transition(ChildWorkflowStatus::Running, Some("started".into()));
    run.updated_at = Utc::now() - chrono::Duration::seconds(301);
    let policy = ChildStalePolicy {
        idle_timeout_secs: 300,
        action: ChildStalePolicyAction::MarkStale,
        ..ChildStalePolicy::default()
    };

    let decision = run.stale_policy_decision(&policy, Utc::now(), &ChildWorkflowHealth::default());
    assert!(matches!(
        decision,
        ChildWorkflowPolicyDecision::MarkStale { .. }
    ));
    run.apply_policy_decision(decision);
    assert_eq!(run.status, ChildWorkflowStatus::Stale);
    assert!(run.stale.as_ref().unwrap().reason.contains("idle"));
}

#[test]
fn stale_detection_can_cancel_on_repeated_failures() {
    let mut run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    run.transition(ChildWorkflowStatus::Running, Some("started".into()));
    let policy = ChildStalePolicy {
        repeated_failure_limit: Some(3),
        action: ChildStalePolicyAction::Cancel,
        ..ChildStalePolicy::default()
    };
    let health = ChildWorkflowHealth {
        repeated_failures: 3,
        ..ChildWorkflowHealth::default()
    };

    let decision = run.stale_policy_decision(&policy, Utc::now(), &health);
    assert!(matches!(
        decision,
        ChildWorkflowPolicyDecision::Cancel { .. }
    ));
    run.apply_policy_decision(decision);
    assert_eq!(run.status, ChildWorkflowStatus::Cancelling);
    assert_eq!(
        run.cancellation.as_ref().unwrap().requested_by.as_deref(),
        Some("child-workflow-policy")
    );
}

#[test]
fn stale_detection_notifies_parent_for_waiting_states() {
    let run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    let health = ChildWorkflowHealth {
        waiting_for_approval: true,
        ..ChildWorkflowHealth::default()
    };
    let decision = run.stale_policy_decision(&ChildStalePolicy::default(), Utc::now(), &health);
    assert!(matches!(
        decision,
        ChildWorkflowPolicyDecision::NotifyParent { .. }
    ));
}

#[test]
fn stale_detection_ignores_terminal_children() {
    let mut run = ChildWorkflowRun::new(ChildWorkflowSpec::default());
    run.transition(ChildWorkflowStatus::Done, Some("done".into()));
    run.updated_at = Utc::now() - chrono::Duration::seconds(1000);
    let decision = run.stale_policy_decision(
        &ChildStalePolicy::default(),
        Utc::now(),
        &ChildWorkflowHealth::default(),
    );
    assert_eq!(decision, ChildWorkflowPolicyDecision::Continue);
}

#[test]
fn child_workflow_delegation_uses_verifier_role_contract_and_handoff() {
    let plan = plan_child_workflow(
        &registry(),
        ChildWorkflowRequest {
            id: "child-verify-1".into(),
            role: "verifier".into(),
            title: "Verify parser fix".into(),
            prompt: "Run parser verification".into(),
        },
    )
    .unwrap();

    assert_eq!(plan.contract.role.as_deref(), Some("verifier"));
    assert!(plan.role.readonly);
    assert!(plan.role.autonomy.can_run_commands);
    assert!(plan
        .contract
        .closeout_criteria
        .criteria
        .iter()
        .any(|criterion| criterion.contains("test-output")));
    assert_eq!(plan.evidence_handoff.role, "verifier");
    assert!(plan
        .evidence_handoff
        .required_evidence
        .contains(&"verification-result".into()));
    assert_eq!(
        plan.evidence_handoff.output_schema.as_deref(),
        Some("verification-result")
    );
}

#[test]
fn child_workflow_delegation_uses_reviewer_role_output_expectations() {
    let plan = plan_child_workflow(
        &registry(),
        ChildWorkflowRequest {
            id: "child-review-1".into(),
            role: "reviewer".into(),
            title: "Review diff".into(),
            prompt: "Review the current changes".into(),
        },
    )
    .unwrap();

    assert_eq!(plan.contract.role.as_deref(), Some("reviewer"));
    assert!(plan.role.readonly);
    assert!(plan
        .evidence_handoff
        .required_evidence
        .contains(&"review-findings".into()));
    assert_eq!(
        plan.evidence_handoff.output_required_sections,
        vec!["findings", "positives", "risks"]
    );
}

#[test]
fn child_workflow_delegation_uses_researcher_evidence_metadata() {
    let plan = plan_child_workflow(
        &registry(),
        ChildWorkflowRequest {
            id: "child-research-1".into(),
            role: "researcher".into(),
            title: "Research API".into(),
            prompt: "Find relevant docs".into(),
        },
    )
    .unwrap();

    assert_eq!(plan.contract.role.as_deref(), Some("researcher"));
    assert!(plan
        .evidence_handoff
        .required_evidence
        .contains(&"source-citations".into()));
    assert_eq!(
        plan.evidence_handoff.output_schema.as_deref(),
        Some("research-summary")
    );
}

#[test]
fn child_workflow_delegation_rejects_unknown_role() {
    let err = plan_child_workflow(
        &registry(),
        ChildWorkflowRequest {
            id: "child-missing".into(),
            role: "missing".into(),
            title: "Missing".into(),
            prompt: "Missing".into(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, ChildWorkflowError::UnknownRole(_)));
}
