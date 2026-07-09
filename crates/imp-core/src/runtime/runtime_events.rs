use super::*;

#[test]
fn runtime_state_snapshot_default_is_empty_and_versioned() {
    let snapshot = RuntimeStateSnapshot::default();
    assert_eq!(snapshot.schema_version, RUNTIME_SCHEMA_VERSION);
    assert_eq!(snapshot.phase, RuntimePhase::Idle);
    assert!(snapshot.active_tools.is_empty());
    assert!(snapshot.pending_approvals.is_empty());
    assert!(snapshot.policy_decisions.is_empty());
    assert!(snapshot.verification_gates.is_empty());
    assert!(snapshot.evidence_refs.is_empty());
    assert!(snapshot.final_status.is_none());
    assert!(snapshot.workflow_refs.is_empty());
}

#[test]
fn runtime_state_snapshot_roundtrips_through_json() {
    let mut snapshot = RuntimeStateSnapshot {
        autonomy_mode: Some(AutonomyMode::WorktreeAuto),
        phase: RuntimePhase::Running,
        ..RuntimeStateSnapshot::default()
    };
    snapshot.workflow.run_id = Some("run-1".into());
    snapshot.workflow.contract_summary = Some("Implement the workflow runtime".into());
    snapshot.active_tools.push(RuntimeToolCall {
        id: "tool-1".into(),
        name: "bash".into(),
        status: RuntimeToolStatus::Running,
        summary: Some("cargo test".into()),
        ..RuntimeToolCall::default()
    });
    snapshot.pending_approvals.push(RuntimeApprovalRef {
        id: "approval-1".into(),
        summary: "apply worktree patch".into(),
        ..RuntimeApprovalRef::default()
    });
    snapshot.policy_decisions.push(RuntimePolicyDecision {
        subject: "bash".into(),
        decision: RuntimePolicyDecisionKind::Allow,
        ..RuntimePolicyDecision::default()
    });
    snapshot
        .verification_gates
        .push(VerificationGate::command("test", "cargo test -p imp-core"));
    snapshot.evidence_refs.push(RuntimeArtifactRef {
        kind: "evidence-packet".into(),
        path: ".imp/runs/run-1/evidence.md".into(),
        summary: Some("run evidence".into()),
    });
    snapshot.final_status = Some(RuntimeFinalStatus::DoneWithConcerns {
        concerns: vec!["unrelated warning".into()],
    });
    snapshot.workflow_refs.push(RuntimeManaRef {
        id: "394.11.2".into(),
        title: Some("Define runtime types".into()),
        status: Some("in_progress".into()),
        url: None,
    });

    let encoded = serde_json::to_string(&snapshot).unwrap();
    let decoded: RuntimeStateSnapshot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
}

#[test]
fn runtime_state_accumulator_tracks_unknown_events_without_corrupting_state() {
    let mut accumulator = RuntimeStateAccumulator::new("run-1");
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        kind: RuntimeEventKind::Unknown {
            name: "future_event".into(),
        },
        ..RuntimeEvent::default()
    });
    let snapshot = accumulator.snapshot();
    assert_eq!(snapshot.phase, RuntimePhase::Idle);
    assert_eq!(
        snapshot
            .status_items
            .get("last-unknown-event")
            .map(String::as_str),
        Some("future_event")
    );
}

#[test]
fn browser_approval_events_update_runtime_state() {
    let mut accumulator = RuntimeStateAccumulator::new("run-1");
    let mut browser = BrowserEvent::new(BrowserEventKind::InputRequested);
    browser.session_id = Some("browser-1".into());
    browser.action = Some("click".into());
    browser.domain = Some("example.com".into());
    accumulator.apply(&RuntimeEvent {
        kind: RuntimeEventKind::BrowserUpdated {
            event: browser.clone(),
        },
        ..RuntimeEvent::default()
    });
    assert_eq!(
        accumulator.snapshot().phase,
        RuntimePhase::WaitingForApproval
    );
    assert_eq!(accumulator.snapshot().pending_approvals.len(), 1);

    browser.kind = BrowserEventKind::InputApproved;
    browser.approval_scope = Some("domain".into());
    accumulator.apply(&RuntimeEvent {
        kind: RuntimeEventKind::BrowserUpdated { event: browser },
        ..RuntimeEvent::default()
    });
    assert_eq!(accumulator.snapshot().phase, RuntimePhase::Running);
    assert!(accumulator.snapshot().pending_approvals.is_empty());
}

#[test]
fn runtime_event_kind_names_are_stable_json_contract() {
    let cases = [
        (
            RuntimeEventKind::AgentStarted { model: "m".into() },
            "agent_started",
        ),
        (
            RuntimeEventKind::MessageDelta {
                delta: "hello".into(),
            },
            "message_delta",
        ),
        (
            RuntimeEventKind::ToolOutput {
                tool_call_id: "tool-1".into(),
                output_delta: "ok".into(),
            },
            "tool_output",
        ),
        (
            RuntimeEventKind::BrowserUpdated {
                event: crate::agent::BrowserEvent::new(
                    crate::agent::BrowserEventKind::SessionStarted,
                ),
            },
            "browser_updated",
        ),
        (
            RuntimeEventKind::WorktreeUpdated {
                worktree: RuntimeWorktreeState::default(),
            },
            "worktree_updated",
        ),
        (
            RuntimeEventKind::EvidenceUpdated {
                artifact: RuntimeArtifactRef {
                    kind: "evidence-packet".into(),
                    path: ".imp/runs/run-1/evidence.md".into(),
                    summary: None,
                },
            },
            "evidence_updated",
        ),
        (
            RuntimeEventKind::Unknown {
                name: "future".into(),
            },
            "unknown",
        ),
    ];

    for (kind, expected_type) in cases {
        let event = RuntimeEvent {
            run_id: "run-1".into(),
            kind,
            ..RuntimeEvent::default()
        };
        let value = serde_json::to_value(&event).expect("runtime event json");
        assert_eq!(value["kind"]["type"], expected_type);
    }
}

#[test]
fn runtime_state_snapshot_replay_fixture_is_stable() {
    let mut accumulator = RuntimeStateAccumulator::new("run-fixture");
    let events = vec![
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 1,
            kind: RuntimeEventKind::AgentStarted {
                model: "openrouter/test".into(),
            },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 2,
            kind: RuntimeEventKind::TurnStarted { index: 1 },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 3,
            kind: RuntimeEventKind::MessageDelta {
                delta: "Working".into(),
            },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 4,
            kind: RuntimeEventKind::ToolStarted {
                tool_call: RuntimeToolCall {
                    id: "tool-1".into(),
                    name: "bash".into(),
                    status: RuntimeToolStatus::Running,
                    args_preview: Some("cargo test".into()),
                    ..RuntimeToolCall::default()
                },
            },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 5,
            kind: RuntimeEventKind::ToolCompleted {
                tool_call: RuntimeToolCall {
                    id: "tool-1".into(),
                    name: "bash".into(),
                    status: RuntimeToolStatus::Succeeded,
                    output_preview: Some("ok".into()),
                    ..RuntimeToolCall::default()
                },
            },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 6,
            kind: RuntimeEventKind::VerificationUpdated {
                gate: VerificationGate::command("test", "cargo test -p imp-core"),
            },
            ..RuntimeEvent::default()
        },
        RuntimeEvent {
            run_id: "run-fixture".into(),
            sequence: 7,
            kind: RuntimeEventKind::AgentEnded {
                status: RuntimeFinalStatus::Done,
                usage: None,
            },
            ..RuntimeEvent::default()
        },
    ];
    for event in &events {
        accumulator.apply(event);
    }

    let snapshot = accumulator.snapshot();
    let value = serde_json::to_value(&snapshot).expect("snapshot json");
    assert_eq!(value["schema_version"], RUNTIME_SCHEMA_VERSION);
    assert_eq!(value["workflow"]["run_id"], "run-fixture");
    assert_eq!(value["workflow"]["model"], "openrouter/test");
    assert_eq!(value["phase"], "completed");
    assert_eq!(value["active_tools"].as_array().unwrap().len(), 0);
    assert_eq!(value["completed_tools"].as_array().unwrap().len(), 1);
    assert_eq!(value["completed_tools"][0]["name"], "bash");
    assert_eq!(value["verification_gates"].as_array().unwrap().len(), 1);
    assert_eq!(value["status_items"]["turn"], "1");
    assert_eq!(value["status_items"]["phase"], "completed");
}

#[test]
fn runtime_event_roundtrips_through_json() {
    let event = RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 7,
        kind: RuntimeEventKind::ToolStarted {
            tool_call: RuntimeToolCall {
                id: "tool-1".into(),
                name: "read".into(),
                status: RuntimeToolStatus::Running,
                ..RuntimeToolCall::default()
            },
        },
        ..RuntimeEvent::default()
    };

    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: RuntimeEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn runtime_state_accumulator_reduces_representative_stream() {
    let mut accumulator = RuntimeStateAccumulator::new("run-1");
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 1,
        kind: RuntimeEventKind::AgentStarted {
            model: "openai/test".into(),
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 2,
        kind: RuntimeEventKind::ToolStarted {
            tool_call: RuntimeToolCall {
                id: "tool-1".into(),
                name: "bash".into(),
                status: RuntimeToolStatus::Running,
                ..RuntimeToolCall::default()
            },
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 3,
        kind: RuntimeEventKind::ToolOutput {
            tool_call_id: "tool-1".into(),
            output_delta: "ok".into(),
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 4,
        kind: RuntimeEventKind::ToolCompleted {
            tool_call: RuntimeToolCall {
                id: "tool-1".into(),
                name: "bash".into(),
                status: RuntimeToolStatus::Succeeded,
                output_preview: Some("ok".into()),
                ..RuntimeToolCall::default()
            },
        },
        ..RuntimeEvent::default()
    });
    let metadata = WorktreeRunMetadata {
        worktree_path: "/tmp/imp-worktree".into(),
        branch: "imp/run/test".into(),
        patch_path: "/tmp/imp-worktree.patch".into(),
        clean: false,
        ..WorktreeRunMetadata::default()
    };
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 5,
        kind: RuntimeEventKind::WorktreeUpdated {
            worktree: RuntimeWorktreeState {
                metadata: metadata.clone(),
                ..RuntimeWorktreeState::default()
            },
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 6,
        kind: RuntimeEventKind::EvidenceUpdated {
            artifact: RuntimeArtifactRef {
                kind: "worktree-diff".into(),
                path: metadata.patch_path.clone(),
                summary: Some("patch".into()),
            },
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 7,
        kind: RuntimeEventKind::PolicyDecision {
            decision: RuntimePolicyDecision {
                subject: "bash".into(),
                decision: RuntimePolicyDecisionKind::Warn,
                reason: Some("review".into()),
                ..RuntimePolicyDecision::default()
            },
        },
        ..RuntimeEvent::default()
    });
    accumulator.apply(&RuntimeEvent {
        run_id: "run-1".into(),
        sequence: 8,
        kind: RuntimeEventKind::AgentEnded {
            status: RuntimeFinalStatus::Done,
            usage: Some(RuntimeUsageSummary {
                total_tokens: 12,
                total_cost: Some("0.001".into()),
                ..RuntimeUsageSummary::default()
            }),
        },
        ..RuntimeEvent::default()
    });

    let snapshot = accumulator.snapshot();
    assert_eq!(snapshot.workflow.run_id.as_deref(), Some("run-1"));
    assert_eq!(snapshot.workflow.model.as_deref(), Some("openai/test"));
    assert_eq!(snapshot.phase, RuntimePhase::Completed);
    assert!(snapshot.active_tools.is_empty());
    assert_eq!(snapshot.completed_tools.len(), 1);
    assert_eq!(snapshot.completed_tools[0].name, "bash");
    assert!(matches!(
        snapshot.final_status,
        Some(RuntimeFinalStatus::Done)
    ));
    assert!(matches!(
        snapshot.workspace.scope,
        WorkspaceScope::Worktree { .. }
    ));
    assert_eq!(
        snapshot
            .workspace
            .worktree
            .as_ref()
            .map(|worktree| worktree.metadata.branch.as_str()),
        Some("imp/run/test")
    );
    assert_eq!(snapshot.evidence_refs.len(), 1);
    assert_eq!(snapshot.policy_decisions.len(), 1);
    assert_eq!(
        snapshot.status_items.get("tokens").map(String::as_str),
        Some("12")
    );
    assert_eq!(
        snapshot.status_items.get("cost").map(String::as_str),
        Some("0.001")
    );
}
#[test]
fn runtime_child_workflow_event_updates_snapshot_and_evidence() {
    let mut run = ChildWorkflowRun::new(crate::workflow::ChildWorkflowSpec {
        id: crate::workflow::ChildWorkflowId::new("child-verifier-1"),
        parent: crate::workflow::ParentWorkflowRef {
            workflow_id: Some("parent-workflow".into()),
            ..crate::workflow::ParentWorkflowRef::default()
        },
        role: "verifier".into(),
        title: "Verify parser".into(),
        prompt: "Run tests".into(),
        ..crate::workflow::ChildWorkflowSpec::default()
    });
    run.status = ChildWorkflowStatus::DoneWithConcerns;
    run.summary = Some(crate::workflow::ChildWorkflowSummary {
        status: ChildWorkflowStatus::DoneWithConcerns,
        summary: "verification completed with failures".into(),
        findings: vec!["parser_empty_input failed".into()],
        concerns: vec!["fix parser".into()],
    });
    run.evidence_refs.push(crate::workflow::ChildEvidenceRef {
        kind: "test-output".into(),
        path: ".imp/runs/parent/children/child-verifier-1/evidence.md".into(),
        summary: Some("test output".into()),
    });

    let mut accumulator = RuntimeStateAccumulator::new("parent-run");
    accumulator.apply(&RuntimeEvent {
        run_id: "parent-run".into(),
        sequence: 1,
        kind: RuntimeEventKind::ChildWorkflowUpdated {
            child: RuntimeChildWorkflowSummary::from_child_run(&run),
        },
        ..RuntimeEvent::default()
    });

    let snapshot = accumulator.snapshot();
    assert_eq!(snapshot.child_workflows.len(), 1);
    assert_eq!(snapshot.child_workflows[0].id, "child-verifier-1");
    assert_eq!(snapshot.child_workflows[0].role, "verifier");
    assert_eq!(
        snapshot.child_workflows[0].status,
        ChildWorkflowStatus::DoneWithConcerns
    );
    assert_eq!(snapshot.phase, RuntimePhase::Completed);
    assert_eq!(snapshot.evidence_refs.len(), 1);
    assert_eq!(
        snapshot
            .status_items
            .get("child-workflow")
            .map(String::as_str),
        Some("child-verifier-1:DoneWithConcerns")
    );
}
