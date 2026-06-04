use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewState {
    NoChange,
    Changed,
    NeedsDecision,
}

impl WorkflowReviewState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoChange => "no_change",
            Self::Changed => "changed",
            Self::NeedsDecision => "needs_decision",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewScopeKind {
    None,
    Project,
    Root,
    ExplicitPath,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowReviewScope {
    pub kind: WorkflowReviewScopeKind,
    pub display: String,
}

impl Default for WorkflowReviewScope {
    fn default() -> Self {
        Self {
            kind: WorkflowReviewScopeKind::None,
            display: "none".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowUnitRef {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl WorkflowUnitRef {
    pub fn from_snapshot(unit: &WorkflowUnitSnapshot) -> Self {
        Self {
            id: unit.id.clone(),
            title: unit.title.clone(),
            kind: Some(unit.kind.as_str().to_string()),
        }
    }

    pub fn new(id: impl Into<String>, title: impl Into<String>, kind: Option<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowUnitSnapshot {
    pub id: String,
    pub title: String,
    pub kind: WorkflowReviewUnitKind,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub priority: u8,
    pub is_archived: bool,
}

impl WorkflowUnitSnapshot {
    pub fn unit_ref(&self) -> WorkflowUnitRef {
        WorkflowUnitRef::from_snapshot(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewUnitKind {
    Epic,
    Job,
    Fact,
}

impl WorkflowReviewUnitKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epic => "epic",
            Self::Job => "job",
            Self::Fact => "fact",
        }
    }

    pub fn from_unit_kind(kind: &str) -> Self {
        match kind {
            "epic" => Self::Epic,
            "fact" => Self::Fact,
            _ => Self::Job,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMutationAction {
    Create,
    Close,
    Update,
    NotesAppend,
    DecisionAdd,
    DecisionResolve,
    Reopen,
    Fail,
    Delete,
    DepAdd,
    DepRemove,
    FactCreate,
}

impl WorkflowMutationAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Close => "close",
            Self::Update => "update",
            Self::NotesAppend => "notes_append",
            Self::DecisionAdd => "decision_add",
            Self::DecisionResolve => "decision_resolve",
            Self::Reopen => "reopen",
            Self::Fail => "fail",
            Self::Delete => "delete",
            Self::DepAdd => "dep_add",
            Self::DepRemove => "dep_remove",
            Self::FactCreate => "fact_create",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaTouchKind {
    Created,
    Updated,
    Closed,
    Reopened,
    Failed,
    Deleted,
    FactCreated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowUnitOrigin {
    Preexisting,
    CreatedInTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowUnitRole {
    Anchor,
    Child,
    Fact,
    DirectTarget,
    Related,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaAnchorKind {
    ReusedExisting,
    CreatedInTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaAnchorReason {
    AttachedParent,
    CreatedParent,
    PrimaryTarget,
    PrimaryFact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowAnchorUnit {
    pub unit: WorkflowUnitRef,
    pub anchor_kind: ManaAnchorKind,
    pub reason: ManaAnchorReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowTouchedUnit {
    pub unit: WorkflowUnitRef,
    pub touch_kind: ManaTouchKind,
    pub unit_origin: WorkflowUnitOrigin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<WorkflowUnitRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowProposedChild {
    pub unit: WorkflowUnitRef,
    pub parent: WorkflowUnitRef,
    pub child_kind: WorkflowReviewUnitKind,
    pub child_origin: WorkflowUnitOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaFieldChangeKind {
    Set,
    Added,
    Removed,
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowFieldChange {
    pub unit: WorkflowUnitRef,
    pub field: String,
    pub change_kind: ManaFieldChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub source_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowNoteAppend {
    pub unit: WorkflowUnitRef,
    pub appended_text: String,
    pub source_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaDecisionEventKind {
    Added,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowDecisionEvent {
    pub unit: WorkflowUnitRef,
    pub event_kind: ManaDecisionEventKind,
    pub decision_text: String,
    pub source_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManaConsequentialChoiceCategory {
    OwnershipBoundary,
    Architecture,
    ExecutionLaunch,
    ScopeChange,
    PruneOrDelete,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowConsequentialChoice {
    pub unit: WorkflowUnitRef,
    pub decision_text: String,
    pub category: ManaConsequentialChoiceCategory,
    pub why_consequential: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_question: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkflowReview {
    pub turn_index: u32,
    pub state: WorkflowReviewState,
    pub scope: WorkflowReviewScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_unit: Option<TurnWorkflowAnchorUnit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_units: Vec<TurnWorkflowTouchedUnit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed_children: Vec<TurnWorkflowProposedChild>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub material_field_changes: Vec<TurnWorkflowFieldChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes_appended: Vec<TurnWorkflowNoteAppend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_events: Vec<TurnWorkflowDecisionEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_consequential_choices: Vec<TurnWorkflowConsequentialChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_question: Option<String>,
}

impl TurnWorkflowReview {
    pub fn no_change(turn_index: u32) -> Self {
        Self {
            turn_index,
            state: WorkflowReviewState::NoChange,
            scope: WorkflowReviewScope::default(),
            anchor_unit: None,
            touched_units: Vec::new(),
            proposed_children: Vec::new(),
            material_field_changes: Vec::new(),
            notes_appended: Vec::new(),
            decision_events: Vec::new(),
            unresolved_consequential_choices: Vec::new(),
            next_question: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowMutationRecord {
    pub action: WorkflowMutationAction,
    pub scope: WorkflowReviewScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_unit: Option<WorkflowUnitSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_unit: Option<WorkflowUnitSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_unit: Option<WorkflowUnitRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_unit: Option<WorkflowUnitRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_unit: Option<WorkflowUnitRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_changes: Vec<TurnWorkflowFieldChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes_appended: Vec<TurnWorkflowNoteAppend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_events: Vec<TurnWorkflowDecisionEvent>,
}

#[derive(Debug, Default)]
pub struct TurnWorkflowReviewAccumulator {
    turn_index: u32,
    mutations: Vec<WorkflowMutationRecord>,
}

impl TurnWorkflowReviewAccumulator {
    pub fn begin_turn(&mut self, turn_index: u32) {
        self.turn_index = turn_index;
        self.mutations.clear();
    }

    pub fn push(&mut self, record: WorkflowMutationRecord) {
        self.mutations.push(record);
    }

    pub fn finalize(&self) -> TurnWorkflowReview {
        if self.mutations.is_empty() {
            return TurnWorkflowReview::no_change(self.turn_index);
        }

        let scope = summarize_scope(&self.mutations);
        let mut aggregates: BTreeMap<String, UnitAggregate> = BTreeMap::new();

        for mutation in &self.mutations {
            match mutation.action {
                WorkflowMutationAction::Delete => {
                    if let Some(unit) = &mutation.deleted_unit {
                        let aggregate = aggregates
                            .entry(unit.id.clone())
                            .or_insert_with(|| UnitAggregate::new(unit.clone()));
                        aggregate.deleted_in_turn = true;
                        aggregate.touch_actions.insert(ManaTouchKind::Deleted);
                        aggregate.roles.insert(WorkflowUnitRole::DirectTarget);
                    }
                }
                _ => {
                    let Some(unit) = mutation
                        .after_unit
                        .as_ref()
                        .or(mutation.before_unit.as_ref())
                    else {
                        continue;
                    };
                    let unit_ref = unit.unit_ref();
                    let aggregate = aggregates
                        .entry(unit.id.clone())
                        .or_insert_with(|| UnitAggregate::new(unit_ref));

                    if aggregate.first_before.is_none() {
                        aggregate.first_before = mutation.before_unit.clone();
                    }
                    if mutation.before_unit.is_some() && aggregate.first_before.is_none() {
                        aggregate.first_before = mutation.before_unit.clone();
                    }
                    if let Some(before) = &mutation.before_unit {
                        aggregate.first_before.get_or_insert_with(|| before.clone());
                    }
                    if let Some(after) = &mutation.after_unit {
                        aggregate.latest_after = Some(after.clone());
                    }
                    aggregate.roles.insert(WorkflowUnitRole::DirectTarget);
                    if matches!(unit.kind, WorkflowReviewUnitKind::Fact) {
                        aggregate.roles.insert(WorkflowUnitRole::Fact);
                    }
                    if let Some(parent) = &mutation.parent_unit {
                        aggregate.parent_unit = Some(parent.clone());
                        aggregate.roles.insert(WorkflowUnitRole::Child);
                    } else if let Some(parent_id) = unit.parent.as_ref() {
                        aggregate.parent_unit.get_or_insert_with(|| {
                            WorkflowUnitRef::new(
                                parent_id.clone(),
                                parent_id.clone(),
                                Some("epic".into()),
                            )
                        });
                        aggregate.roles.insert(WorkflowUnitRole::Child);
                    }

                    match mutation.action {
                        WorkflowMutationAction::Create => {
                            aggregate.created_in_turn = true;
                            aggregate.touch_actions.insert(ManaTouchKind::Created);
                        }
                        WorkflowMutationAction::FactCreate => {
                            aggregate.created_in_turn = true;
                            aggregate.touch_actions.insert(ManaTouchKind::FactCreated);
                            aggregate.roles.insert(WorkflowUnitRole::Fact);
                        }
                        WorkflowMutationAction::Close => {
                            aggregate.touch_actions.insert(ManaTouchKind::Closed);
                        }
                        WorkflowMutationAction::Reopen => {
                            aggregate.touch_actions.insert(ManaTouchKind::Reopened);
                        }
                        WorkflowMutationAction::Fail => {
                            aggregate.touch_actions.insert(ManaTouchKind::Failed);
                        }
                        _ => {
                            aggregate.touch_actions.insert(ManaTouchKind::Updated);
                        }
                    }
                }
            }

            if let Some(related) = &mutation.related_unit {
                let aggregate = aggregates
                    .entry(related.id.clone())
                    .or_insert_with(|| UnitAggregate::new(related.clone()));
                aggregate.roles.insert(WorkflowUnitRole::Related);
            }

            for field_change in &mutation.field_changes {
                if let Some(aggregate) = aggregates.get_mut(&field_change.unit.id) {
                    aggregate.record_field_change(field_change.clone());
                }
            }
            for note in &mutation.notes_appended {
                if let Some(aggregate) = aggregates.get_mut(&note.unit.id) {
                    aggregate.notes.push(note.clone());
                }
            }
            for event in &mutation.decision_events {
                if let Some(aggregate) = aggregates.get_mut(&event.unit.id) {
                    aggregate.record_decision_event(event.clone());
                }
            }
        }

        let created_and_deleted: BTreeSet<String> = aggregates
            .iter()
            .filter_map(|(id, aggregate)| {
                (aggregate.created_in_turn && aggregate.deleted_in_turn).then_some(id.clone())
            })
            .collect();

        let mut touched_units = Vec::new();
        let mut proposed_children = Vec::new();
        let mut material_field_changes = Vec::new();
        let mut notes_appended = Vec::new();
        let mut decision_events = Vec::new();
        let mut unresolved_consequential_choices = Vec::new();

        for (id, aggregate) in &aggregates {
            if created_and_deleted.contains(id) {
                continue;
            }

            let surviving_field_changes = aggregate.surviving_field_changes();
            let surviving_decision_events = aggregate.surviving_decision_events();
            let final_unit = aggregate.latest_after.as_ref();
            let unresolved_choices = final_unit
                .map(classify_unresolved_choices)
                .unwrap_or_default();

            let has_surviving_material = aggregate.created_in_turn
                || aggregate.deleted_in_turn
                || !surviving_field_changes.is_empty()
                || !aggregate.notes.is_empty()
                || !surviving_decision_events.is_empty();

            if has_surviving_material {
                let unit_ref = aggregate.display_unit_ref();
                touched_units.push(TurnWorkflowTouchedUnit {
                    unit: unit_ref.clone(),
                    touch_kind: aggregate.touch_kind(),
                    unit_origin: if aggregate.created_in_turn {
                        WorkflowUnitOrigin::CreatedInTurn
                    } else {
                        WorkflowUnitOrigin::Preexisting
                    },
                    roles: aggregate.roles.iter().copied().collect(),
                });

                if aggregate.created_in_turn {
                    if let (Some(parent), Some(after)) = (&aggregate.parent_unit, final_unit) {
                        if matches!(
                            after.kind,
                            WorkflowReviewUnitKind::Epic | WorkflowReviewUnitKind::Job
                        ) {
                            proposed_children.push(TurnWorkflowProposedChild {
                                unit: unit_ref,
                                parent: parent.clone(),
                                child_kind: after.kind,
                                child_origin: WorkflowUnitOrigin::CreatedInTurn,
                            });
                        }
                    }
                }
            }

            material_field_changes.extend(surviving_field_changes);
            notes_appended.extend(aggregate.notes.clone());
            decision_events.extend(surviving_decision_events);
            unresolved_consequential_choices.extend(unresolved_choices);
        }

        touched_units.sort_by(|a, b| a.unit.id.cmp(&b.unit.id));
        proposed_children.sort_by(|a, b| a.unit.id.cmp(&b.unit.id));
        material_field_changes.sort_by(|a, b| {
            a.unit
                .id
                .cmp(&b.unit.id)
                .then(a.field.cmp(&b.field))
                .then(a.source_action.cmp(&b.source_action))
        });
        notes_appended.sort_by(|a, b| a.unit.id.cmp(&b.unit.id));
        decision_events.sort_by(|a, b| {
            a.unit
                .id
                .cmp(&b.unit.id)
                .then(a.decision_text.cmp(&b.decision_text))
        });
        unresolved_consequential_choices.sort_by(|a, b| {
            a.unit
                .id
                .cmp(&b.unit.id)
                .then(a.decision_text.cmp(&b.decision_text))
        });

        let anchor_unit = derive_anchor_unit(
            &aggregates,
            &created_and_deleted,
            &proposed_children,
            &touched_units,
        );
        let next_question = unresolved_consequential_choices
            .first()
            .and_then(|choice| choice.suggested_question.clone())
            .or_else(|| {
                unresolved_consequential_choices
                    .first()
                    .map(|choice| format!("{} · {}", choice.unit.id, choice.decision_text))
            });

        let state = if touched_units.is_empty()
            && proposed_children.is_empty()
            && material_field_changes.is_empty()
            && notes_appended.is_empty()
            && decision_events.is_empty()
            && unresolved_consequential_choices.is_empty()
        {
            WorkflowReviewState::NoChange
        } else if unresolved_consequential_choices.is_empty() {
            WorkflowReviewState::Changed
        } else {
            WorkflowReviewState::NeedsDecision
        };

        TurnWorkflowReview {
            turn_index: self.turn_index,
            state,
            scope,
            anchor_unit,
            touched_units,
            proposed_children,
            material_field_changes,
            notes_appended,
            decision_events,
            unresolved_consequential_choices,
            next_question,
        }
    }
}

#[derive(Debug, Clone)]
struct FieldAggregate {
    unit: WorkflowUnitRef,
    field: String,
    change_kind: ManaFieldChangeKind,
    before: Option<String>,
    after: Option<String>,
    source_action: String,
}

#[derive(Debug, Clone)]
struct UnitAggregate {
    display_unit: WorkflowUnitRef,
    first_before: Option<WorkflowUnitSnapshot>,
    latest_after: Option<WorkflowUnitSnapshot>,
    parent_unit: Option<WorkflowUnitRef>,
    created_in_turn: bool,
    deleted_in_turn: bool,
    touch_actions: BTreeSet<ManaTouchKind>,
    roles: BTreeSet<WorkflowUnitRole>,
    singular_field_changes: BTreeMap<String, FieldAggregate>,
    label_changes: BTreeMap<String, i32>,
    dependency_changes: BTreeMap<String, i32>,
    notes: Vec<TurnWorkflowNoteAppend>,
    decision_added: HashMap<String, usize>,
    decision_resolved: HashMap<String, usize>,
    decision_event_order: Vec<TurnWorkflowDecisionEvent>,
}

impl UnitAggregate {
    fn new(display_unit: WorkflowUnitRef) -> Self {
        Self {
            display_unit,
            first_before: None,
            latest_after: None,
            parent_unit: None,
            created_in_turn: false,
            deleted_in_turn: false,
            touch_actions: BTreeSet::new(),
            roles: BTreeSet::new(),
            singular_field_changes: BTreeMap::new(),
            label_changes: BTreeMap::new(),
            dependency_changes: BTreeMap::new(),
            notes: Vec::new(),
            decision_added: HashMap::new(),
            decision_resolved: HashMap::new(),
            decision_event_order: Vec::new(),
        }
    }

    fn display_unit_ref(&self) -> WorkflowUnitRef {
        self.latest_after
            .as_ref()
            .map(WorkflowUnitSnapshot::unit_ref)
            .unwrap_or_else(|| self.display_unit.clone())
    }

    fn record_field_change(&mut self, change: TurnWorkflowFieldChange) {
        match change.field.as_str() {
            "labels" => {
                if let Some(label) = change.after.clone().or(change.before.clone()) {
                    let delta = match change.change_kind {
                        ManaFieldChangeKind::Added => 1,
                        ManaFieldChangeKind::Removed => -1,
                        _ => 0,
                    };
                    *self.label_changes.entry(label).or_insert(0) += delta;
                }
            }
            "dependencies" => {
                if let Some(dep) = change.after.clone().or(change.before.clone()) {
                    let delta = match change.change_kind {
                        ManaFieldChangeKind::Added => 1,
                        ManaFieldChangeKind::Removed => -1,
                        _ => 0,
                    };
                    *self.dependency_changes.entry(dep).or_insert(0) += delta;
                }
            }
            _ => {
                let key = change.field.clone();
                self.singular_field_changes
                    .entry(key)
                    .and_modify(|entry| {
                        entry.after = change.after.clone();
                        entry.change_kind = change.change_kind;
                        entry.source_action = change.source_action.clone();
                    })
                    .or_insert(FieldAggregate {
                        unit: change.unit,
                        field: change.field,
                        change_kind: change.change_kind,
                        before: change.before,
                        after: change.after,
                        source_action: change.source_action,
                    });
            }
        }
    }

    fn record_decision_event(&mut self, event: TurnWorkflowDecisionEvent) {
        match event.event_kind {
            ManaDecisionEventKind::Added => {
                *self
                    .decision_added
                    .entry(event.decision_text.clone())
                    .or_insert(0) += 1;
            }
            ManaDecisionEventKind::Resolved => {
                *self
                    .decision_resolved
                    .entry(event.decision_text.clone())
                    .or_insert(0) += 1;
            }
        }
        self.decision_event_order.push(event);
    }

    fn surviving_decision_events(&self) -> Vec<TurnWorkflowDecisionEvent> {
        let mut remaining_added = self.decision_added.clone();
        let mut remaining_resolved = self.decision_resolved.clone();

        for (decision, added_count) in &self.decision_added {
            if let Some(resolved_count) = remaining_resolved.get_mut(decision) {
                let cancelled = (*added_count).min(*resolved_count);
                if let Some(added_remaining) = remaining_added.get_mut(decision) {
                    *added_remaining -= cancelled;
                }
                *resolved_count -= cancelled;
            }
        }

        self.decision_event_order
            .iter()
            .filter(|event| match event.event_kind {
                ManaDecisionEventKind::Added => {
                    remaining_added
                        .get(&event.decision_text)
                        .copied()
                        .unwrap_or_default()
                        > 0
                }
                ManaDecisionEventKind::Resolved => {
                    remaining_resolved
                        .get(&event.decision_text)
                        .copied()
                        .unwrap_or_default()
                        > 0
                }
            })
            .cloned()
            .collect()
    }

    fn surviving_field_changes(&self) -> Vec<TurnWorkflowFieldChange> {
        let mut out = Vec::new();

        for aggregate in self.singular_field_changes.values() {
            if aggregate.before == aggregate.after {
                continue;
            }
            out.push(TurnWorkflowFieldChange {
                unit: aggregate.unit.clone(),
                field: aggregate.field.clone(),
                change_kind: aggregate.change_kind,
                before: aggregate.before.clone(),
                after: aggregate.after.clone(),
                source_action: aggregate.source_action.clone(),
            });
        }

        for (label, delta) in &self.label_changes {
            if *delta > 0 {
                out.push(TurnWorkflowFieldChange {
                    unit: self.display_unit_ref(),
                    field: "labels".to_string(),
                    change_kind: ManaFieldChangeKind::Added,
                    before: None,
                    after: Some(label.clone()),
                    source_action: WorkflowMutationAction::Update.as_str().to_string(),
                });
            } else if *delta < 0 {
                out.push(TurnWorkflowFieldChange {
                    unit: self.display_unit_ref(),
                    field: "labels".to_string(),
                    change_kind: ManaFieldChangeKind::Removed,
                    before: Some(label.clone()),
                    after: None,
                    source_action: WorkflowMutationAction::Update.as_str().to_string(),
                });
            }
        }

        for (dep, delta) in &self.dependency_changes {
            if *delta > 0 {
                out.push(TurnWorkflowFieldChange {
                    unit: self.display_unit_ref(),
                    field: "dependencies".to_string(),
                    change_kind: ManaFieldChangeKind::Added,
                    before: None,
                    after: Some(dep.clone()),
                    source_action: WorkflowMutationAction::DepAdd.as_str().to_string(),
                });
            } else if *delta < 0 {
                out.push(TurnWorkflowFieldChange {
                    unit: self.display_unit_ref(),
                    field: "dependencies".to_string(),
                    change_kind: ManaFieldChangeKind::Removed,
                    before: Some(dep.clone()),
                    after: None,
                    source_action: WorkflowMutationAction::DepRemove.as_str().to_string(),
                });
            }
        }

        out
    }

    fn touch_kind(&self) -> ManaTouchKind {
        if self.deleted_in_turn {
            ManaTouchKind::Deleted
        } else if self.created_in_turn {
            match self.latest_after.as_ref().map(|unit| unit.kind) {
                Some(WorkflowReviewUnitKind::Fact) => ManaTouchKind::FactCreated,
                _ => ManaTouchKind::Created,
            }
        } else if self.touch_actions.contains(&ManaTouchKind::Failed) {
            ManaTouchKind::Failed
        } else if self.touch_actions.contains(&ManaTouchKind::Reopened) {
            ManaTouchKind::Reopened
        } else if self.touch_actions.contains(&ManaTouchKind::Closed) {
            ManaTouchKind::Closed
        } else {
            ManaTouchKind::Updated
        }
    }
}

fn summarize_scope(mutations: &[WorkflowMutationRecord]) -> WorkflowReviewScope {
    let unique: BTreeSet<(WorkflowReviewScopeKind, String)> = mutations
        .iter()
        .map(|mutation| (mutation.scope.kind.clone(), mutation.scope.display.clone()))
        .collect();

    if unique.is_empty() {
        return WorkflowReviewScope::default();
    }
    if unique.len() == 1 {
        let (kind, display) = unique.into_iter().next().unwrap();
        return WorkflowReviewScope { kind, display };
    }

    WorkflowReviewScope {
        kind: WorkflowReviewScopeKind::Mixed,
        display: "mixed".to_string(),
    }
}

fn derive_anchor_unit(
    aggregates: &BTreeMap<String, UnitAggregate>,
    created_and_deleted: &BTreeSet<String>,
    proposed_children: &[TurnWorkflowProposedChild],
    touched_units: &[TurnWorkflowTouchedUnit],
) -> Option<TurnWorkflowAnchorUnit> {
    if !proposed_children.is_empty() {
        let mut parent_counts: BTreeMap<String, (&WorkflowUnitRef, usize)> = BTreeMap::new();
        for child in proposed_children {
            parent_counts
                .entry(child.parent.id.clone())
                .and_modify(|(_, count)| *count += 1)
                .or_insert((&child.parent, 1));
        }
        if let Some((parent_id, (parent_ref, _))) = parent_counts
            .into_iter()
            .max_by_key(|(_, (_, count))| *count)
        {
            let created_in_turn = aggregates
                .get(&parent_id)
                .map(|aggregate| {
                    aggregate.created_in_turn && !created_and_deleted.contains(&parent_id)
                })
                .unwrap_or(false);
            return Some(TurnWorkflowAnchorUnit {
                unit: parent_ref.clone(),
                anchor_kind: if created_in_turn {
                    ManaAnchorKind::CreatedInTurn
                } else {
                    ManaAnchorKind::ReusedExisting
                },
                reason: if created_in_turn {
                    ManaAnchorReason::CreatedParent
                } else {
                    ManaAnchorReason::AttachedParent
                },
            });
        }
    }

    if touched_units.len() == 1 {
        let touched = touched_units.first().unwrap();
        let reason = if touched.roles.contains(&WorkflowUnitRole::Fact) {
            ManaAnchorReason::PrimaryFact
        } else {
            ManaAnchorReason::PrimaryTarget
        };
        let anchor_kind = if matches!(touched.unit_origin, WorkflowUnitOrigin::CreatedInTurn) {
            ManaAnchorKind::CreatedInTurn
        } else {
            ManaAnchorKind::ReusedExisting
        };
        return Some(TurnWorkflowAnchorUnit {
            unit: touched.unit.clone(),
            anchor_kind,
            reason,
        });
    }

    None
}

fn classify_unresolved_choices(
    unit: &WorkflowUnitSnapshot,
) -> Vec<TurnWorkflowConsequentialChoice> {
    unit.decisions
        .iter()
        .filter_map(|decision| classify_consequential_choice(&unit.unit_ref(), decision))
        .collect()
}

fn classify_consequential_choice(
    unit: &WorkflowUnitRef,
    decision: &str,
) -> Option<TurnWorkflowConsequentialChoice> {
    let lower = decision.to_ascii_lowercase();

    let (category, why) = if contains_any(
        &lower,
        &[
            "workflow",
            "imp",
            "root",
            "project",
            "ownership",
            "boundary",
        ],
    ) {
        (
            ManaConsequentialChoiceCategory::OwnershipBoundary,
            "changes ownership or storage boundary".to_string(),
        )
    } else if contains_any(
        &lower,
        &["architecture", "design", "contract", "interface", "model"],
    ) {
        (
            ManaConsequentialChoiceCategory::Architecture,
            "changes architecture direction".to_string(),
        )
    } else if contains_any(
        &lower,
        &["launch", "execute", "run", "implement", "start", "ship"],
    ) {
        (
            ManaConsequentialChoiceCategory::ExecutionLaunch,
            "changes whether or how execution should start".to_string(),
        )
    } else if contains_any(&lower, &["scope", "split", "phase", "defer", "cut"]) {
        (
            ManaConsequentialChoiceCategory::ScopeChange,
            "changes preserved scope or decomposition".to_string(),
        )
    } else if contains_any(&lower, &["delete", "prune", "remove", "archive"]) {
        (
            ManaConsequentialChoiceCategory::PruneOrDelete,
            "changes whether captured structure should be removed".to_string(),
        )
    } else if decision.trim_end().ends_with('?')
        || lower.starts_with("choose ")
        || lower.starts_with("decide ")
    {
        (
            ManaConsequentialChoiceCategory::Other,
            "is phrased as an unresolved decision".to_string(),
        )
    } else {
        return None;
    };

    Some(TurnWorkflowConsequentialChoice {
        unit: unit.clone(),
        decision_text: decision.to_string(),
        category,
        why_consequential: why,
        suggested_question: Some(format!("{} · {}", unit.id, decision)),
    })
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
#[path = "workflow_review/tests.rs"]
mod tests;
