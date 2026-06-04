use super::*;

fn scope() -> WorkflowReviewScope {
    WorkflowReviewScope {
        kind: WorkflowReviewScopeKind::Project,
        display: "project".to_string(),
    }
}

fn unit(id: &str, title: &str, kind: WorkflowReviewUnitKind) -> WorkflowUnitSnapshot {
    WorkflowUnitSnapshot {
        id: id.to_string(),
        title: title.to_string(),
        kind,
        status: "open".to_string(),
        parent: None,
        dependencies: Vec::new(),
        labels: Vec::new(),
        decisions: Vec::new(),
        description: None,
        acceptance: None,
        design: None,
        assignee: None,
        priority: 2,
        is_archived: false,
    }
}

#[test]
fn no_mutations_becomes_no_change() {
    let mut acc = TurnWorkflowReviewAccumulator::default();
    acc.begin_turn(3);
    let review = acc.finalize();
    assert_eq!(review.state, WorkflowReviewState::NoChange);
    assert_eq!(review.state.as_str(), "no_change");
}

#[test]
fn create_then_delete_same_unit_is_net_zero() {
    let mut acc = TurnWorkflowReviewAccumulator::default();
    acc.begin_turn(1);
    let created = unit("28.5", "child", WorkflowReviewUnitKind::Job);
    acc.push(WorkflowMutationRecord {
        action: WorkflowMutationAction::Create,
        scope: scope(),
        before_unit: None,
        after_unit: Some(created.clone()),
        deleted_unit: None,
        parent_unit: Some(WorkflowUnitRef::new("28", "parent", Some("epic".into()))),
        related_unit: None,
        field_changes: Vec::new(),
        notes_appended: Vec::new(),
        decision_events: Vec::new(),
    });
    acc.push(WorkflowMutationRecord {
        action: WorkflowMutationAction::Delete,
        scope: scope(),
        before_unit: Some(created.clone()),
        after_unit: None,
        deleted_unit: Some(created.unit_ref()),
        parent_unit: None,
        related_unit: None,
        field_changes: vec![TurnWorkflowFieldChange {
            unit: created.unit_ref(),
            field: "lifecycle.deleted".into(),
            change_kind: ManaFieldChangeKind::Set,
            before: Some("false".into()),
            after: Some("true".into()),
            source_action: "delete".into(),
        }],
        notes_appended: Vec::new(),
        decision_events: Vec::new(),
    });
    let review = acc.finalize();
    assert_eq!(review.state, WorkflowReviewState::NoChange);
    assert!(review.touched_units.is_empty());
}

#[test]
fn unresolved_architecture_decision_requires_decision() {
    let mut acc = TurnWorkflowReviewAccumulator::default();
    acc.begin_turn(2);
    let mut after = unit("28", "boundary work", WorkflowReviewUnitKind::Epic);
    after.decisions = vec!["Choose architecture boundary between workflow and imp".into()];
    acc.push(WorkflowMutationRecord {
        action: WorkflowMutationAction::DecisionAdd,
        scope: scope(),
        before_unit: Some(unit("28", "boundary work", WorkflowReviewUnitKind::Epic)),
        after_unit: Some(after.clone()),
        deleted_unit: None,
        parent_unit: None,
        related_unit: None,
        field_changes: Vec::new(),
        notes_appended: Vec::new(),
        decision_events: vec![TurnWorkflowDecisionEvent {
            unit: after.unit_ref(),
            event_kind: ManaDecisionEventKind::Added,
            decision_text: "Choose architecture boundary between workflow and imp".into(),
            source_action: "decision_add".into(),
        }],
    });
    let review = acc.finalize();
    assert_eq!(review.state, WorkflowReviewState::NeedsDecision);
    assert_eq!(review.unresolved_consequential_choices.len(), 1);
    assert!(review.next_question.is_some());
}
