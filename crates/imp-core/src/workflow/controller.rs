use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::agent::{ContinueReason, RunFinalStatus, StopReason};

/// Runtime-owned state for a workflow-backed workflow run.
///
/// This is intentionally policy-shaped rather than model-shaped: the model may
/// propose work, but the runtime owns whether a workflow is still obligated to
/// inspect, supervise, verify, or close out before presenting the work as complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowRunController {
    pub workflow_id: Option<String>,
    pub workflow_root_id: Option<String>,
    pub active_unit_id: Option<String>,
    pub child_runs: Vec<WorkflowChildRun>,
    pub graph_closeout_required: bool,
    pub direct_closeout_required: bool,
    pub budget: WorkflowRunBudget,
    pub counters: WorkflowRunCounters,
    pub bootstrap: WorkflowBootstrapState,
    pub graph_shape: WorkflowGraphShape,
    pub planning: WorkflowPlanningState,
    pub closeout: WorkflowCloseoutState,
}

impl WorkflowRunController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_workflow_id(mut self, workflow_id: impl Into<String>) -> Self {
        self.workflow_id = Some(workflow_id.into());
        self
    }

    pub fn with_workflow_root_id(mut self, workflow_root_id: impl Into<String>) -> Self {
        let workflow_root_id = workflow_root_id.into();
        self.workflow_root_id = Some(workflow_root_id.clone());
        self.active_unit_id.get_or_insert(workflow_root_id);
        self
    }

    pub fn with_active_unit_id(mut self, active_unit_id: impl Into<String>) -> Self {
        self.active_unit_id = Some(active_unit_id.into());
        self
    }

    pub fn tick_turn(&mut self) {
        self.counters.turns += 1;
        self.counters.last_activity_unix_secs = Some(now_unix_secs());
    }

    pub fn budget_status(&self) -> WorkflowBudgetStatus {
        if let Some(max_turns) = self.budget.max_turns {
            if self.counters.turns >= max_turns {
                return WorkflowBudgetStatus::Exhausted(format!(
                    "workflow turn budget exhausted: {}/{} turns",
                    self.counters.turns, max_turns
                ));
            }
        }

        if let Some(max_idle_secs) = self.budget.max_idle_secs {
            let now = now_unix_secs();
            let last = self
                .counters
                .last_activity_unix_secs
                .unwrap_or(self.counters.started_unix_secs);
            if now.saturating_sub(last) >= max_idle_secs {
                return WorkflowBudgetStatus::Exhausted(format!(
                    "workflow idle budget exhausted: {}s without activity",
                    now.saturating_sub(last)
                ));
            }
        }

        WorkflowBudgetStatus::WithinBudget
    }

    pub fn save_to_path(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn load_from_path(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    pub fn require_bootstrap(&mut self) {
        if matches!(self.bootstrap, WorkflowBootstrapState::Unspecified) {
            self.bootstrap = WorkflowBootstrapState::Required;
        }
    }

    pub fn skip_bootstrap(&mut self, reason: impl Into<String>) {
        self.bootstrap = WorkflowBootstrapState::Skipped {
            reason: reason.into(),
        };
    }

    pub fn bind_workflow_root(&mut self, workflow_root_id: impl Into<String>) {
        let workflow_root_id = workflow_root_id.into();
        self.workflow_root_id = Some(workflow_root_id.clone());
        self.active_unit_id
            .get_or_insert_with(|| workflow_root_id.clone());
        self.bootstrap = WorkflowBootstrapState::Complete { workflow_root_id };
    }

    pub fn bootstrap_required(&self) -> bool {
        matches!(self.bootstrap, WorkflowBootstrapState::Required)
    }

    pub fn set_graph_shape(&mut self, graph_shape: WorkflowGraphShape) {
        self.graph_shape = graph_shape;
        if matches!(graph_shape, WorkflowGraphShape::RootOnly) {
            self.planning = WorkflowPlanningState::RootOnly;
        } else if matches!(graph_shape, WorkflowGraphShape::NeedsDecomposition)
            && !matches!(self.planning, WorkflowPlanningState::Decomposed { .. })
        {
            self.planning = WorkflowPlanningState::AwaitingDecomposition;
        }
    }

    pub fn record_child_unit(&mut self, child_unit_id: impl Into<String>) {
        let child_unit_id = child_unit_id.into();
        match &mut self.planning {
            WorkflowPlanningState::Decomposed {
                child_unit_ids,
                completed_child_unit_ids: _,
            } => {
                if !child_unit_ids.iter().any(|id| id == &child_unit_id) {
                    child_unit_ids.push(child_unit_id.clone());
                }
            }
            _ => {
                self.planning = WorkflowPlanningState::Decomposed {
                    child_unit_ids: vec![child_unit_id.clone()],
                    completed_child_unit_ids: Vec::new(),
                };
            }
        }
        if self.active_unit_id.as_deref() == self.workflow_root_id.as_deref() {
            self.active_unit_id = Some(child_unit_id);
        } else {
            self.active_unit_id.get_or_insert(child_unit_id);
        }
    }

    pub fn complete_unit(&mut self, unit_id: &str) {
        let WorkflowPlanningState::Decomposed {
            child_unit_ids,
            completed_child_unit_ids,
        } = &mut self.planning
        else {
            if self.active_unit_id.as_deref() == Some(unit_id) {
                self.active_unit_id = self.workflow_root_id.clone();
            }
            return;
        };

        if child_unit_ids.iter().any(|id| id == unit_id)
            && !completed_child_unit_ids.iter().any(|id| id == unit_id)
        {
            completed_child_unit_ids.push(unit_id.to_string());
        }

        self.active_unit_id = child_unit_ids
            .iter()
            .find(|id| !completed_child_unit_ids.iter().any(|done| done == *id))
            .cloned()
            .or_else(|| self.workflow_root_id.clone());
    }

    pub fn update_child_run_status(&mut self, run_id: &str, status: WorkflowChildRunStatus) {
        if let Some(child) = self
            .child_runs
            .iter_mut()
            .find(|child| child.run_id == run_id)
        {
            child.status = status;
        } else {
            self.child_runs.push(WorkflowChildRun {
                run_id: run_id.to_string(),
                status,
            });
        }
    }

    pub fn record_workflow_orchestration_started(&mut self, run_id: Option<String>) {
        let Some(run_id) = run_id.filter(|id| !id.trim().is_empty() && id != "unknown") else {
            self.record_workflow_graph_changed();
            return;
        };
        self.update_child_run_status(&run_id, WorkflowChildRunStatus::Running);
    }

    pub fn record_workflow_graph_changed(&mut self) {
        self.graph_closeout_required = true;
        self.closeout.ready = false;
    }

    pub fn record_direct_work_changed(&mut self) {
        self.direct_closeout_required = true;
        self.closeout.ready = false;
    }

    pub fn record_closeout_ready(&mut self) {
        self.graph_closeout_required = false;
        self.direct_closeout_required = false;
        self.closeout.ready = true;
    }

    pub fn closeout_check(&self) -> WorkflowCloseoutCheck {
        let mut remaining = Vec::new();
        let mut blockers = Vec::new();

        for child in &self.child_runs {
            match child.status {
                WorkflowChildRunStatus::Unknown | WorkflowChildRunStatus::Running => remaining
                    .push(format!(
                        "child run {} is not terminal ({:?})",
                        child.run_id, child.status
                    )),
                WorkflowChildRunStatus::Failed => {
                    blockers.push(format!("child run {} failed", child.run_id));
                }
                WorkflowChildRunStatus::Blocked => {
                    blockers.push(format!("child run {} is blocked", child.run_id));
                }
                WorkflowChildRunStatus::Done => {}
            }
        }

        if matches!(self.planning, WorkflowPlanningState::AwaitingDecomposition) {
            remaining.push("workflow needs decomposition into real child work units".to_string());
        }
        if let WorkflowPlanningState::Decomposed {
            child_unit_ids,
            completed_child_unit_ids,
        } = &self.planning
        {
            let incomplete_children: Vec<_> = child_unit_ids
                .iter()
                .filter(|id| !completed_child_unit_ids.iter().any(|done| done == *id))
                .collect();
            if !incomplete_children.is_empty() {
                remaining.push(format!(
                    "{} decomposed child work unit(s) incomplete",
                    incomplete_children.len()
                ));
            }
        }
        if self.graph_closeout_required {
            remaining.push("workflow graph changed and needs closeout inspection".to_string());
        }
        if self.direct_closeout_required {
            remaining.push("direct work changed and needs verification closeout".to_string());
        }
        if !self.closeout.ready {
            remaining.push("workflow closeout checklist is not ready".to_string());
        }
        if self.bootstrap_required() {
            remaining.push("durable workflow bootstrap requires a bound workflow root".to_string());
        }

        if let Some(blocker) = self.closeout.blocker.as_ref() {
            blockers.push(blocker.clone());
        }

        WorkflowCloseoutCheck {
            ready: remaining.is_empty() && blockers.is_empty(),
            remaining,
            blockers,
        }
    }

    pub fn enforce_closeout_status(&self, proposed: RunFinalStatus) -> RunFinalStatus {
        match proposed {
            RunFinalStatus::Done { .. } | RunFinalStatus::DoneWithConcerns { .. } => {}
            RunFinalStatus::Blocked { .. }
            | RunFinalStatus::NeedsUserInput { .. }
            | RunFinalStatus::Cancelled
            | RunFinalStatus::Failed { .. } => return proposed,
        }

        let check = self.closeout_check();
        if check.ready {
            return proposed;
        }
        if !check.blockers.is_empty() {
            return RunFinalStatus::Blocked {
                reason: StopReason::ExecutionBlocked,
                message: check.blockers.join("; "),
            };
        }

        proposed.with_concern(format!(
            "workflow closeout incomplete: {}",
            check.remaining.join("; ")
        ))
    }

    pub fn decide_next(&self) -> WorkflowControllerDecision {
        if self.bootstrap_required() {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_bootstrap_prompt(),
                reason: ContinueReason::WorkflowBootstrap,
            };
        }

        if let WorkflowBudgetStatus::Exhausted(message) = self.budget_status() {
            return WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Blocked {
                    reason: StopReason::ExecutionBlocked,
                    message,
                },
            };
        }

        if let Some(blocker) = self.closeout.blocker.as_ref() {
            return WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Blocked {
                    reason: StopReason::UserBlocker,
                    message: blocker.clone(),
                },
            };
        }

        if self.child_runs.iter().any(|run| {
            matches!(
                run.status,
                WorkflowChildRunStatus::Running | WorkflowChildRunStatus::Unknown
            )
        }) {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_supervision_prompt(),
                reason: ContinueReason::OrchestrationProgress,
            };
        }

        if matches!(self.planning, WorkflowPlanningState::AwaitingDecomposition) {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_decomposition_prompt(),
                reason: ContinueReason::WorkflowDecomposition,
            };
        }

        if self.graph_closeout_required {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_graph_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            };
        }

        if self.direct_closeout_required {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_direct_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            };
        }

        if !self.closeout.ready {
            return WorkflowControllerDecision::Continue {
                prompt: workflow_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            };
        }

        WorkflowControllerDecision::Stop {
            status: RunFinalStatus::Done {
                reason: StopReason::WorkCompleted,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowChildRun {
    pub run_id: String,
    pub status: WorkflowChildRunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowChildRunStatus {
    Unknown,
    Running,
    Done,
    Failed,
    Blocked,
}

impl WorkflowChildRunStatus {
    pub fn from_workflow_run_status(status: &str, total_failed: Option<u64>) -> Self {
        match status {
            "starting" | "running" => Self::Running,
            "failed" | "interrupted" => Self::Failed,
            "blocked" => Self::Blocked,
            "done" | "completed" | "finished" if total_failed.unwrap_or(0) > 0 => Self::Failed,
            "done" | "completed" | "finished" => Self::Done,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCloseoutState {
    pub ready: bool,
    pub blocker: Option<String>,
}

impl Default for WorkflowCloseoutState {
    fn default() -> Self {
        Self {
            ready: true,
            blocker: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WorkflowControllerSnapshot {
    pub workflow_id: Option<String>,
    pub workflow_root_id: Option<String>,
    pub active_unit_id: Option<String>,
    pub child_runs: Vec<WorkflowChildRun>,
    pub graph_closeout_required: bool,
    pub direct_closeout_required: bool,
    pub bootstrap: WorkflowBootstrapState,
    pub graph_shape: WorkflowGraphShape,
    pub planning: WorkflowPlanningState,
    pub closeout_ready: bool,
    pub budget: WorkflowRunBudget,
    pub counters: WorkflowRunCounters,
    pub next_decision: Option<String>,
}

impl WorkflowRunController {
    pub fn snapshot(&self) -> WorkflowControllerSnapshot {
        let next_decision = match self.decide_next() {
            WorkflowControllerDecision::Continue { reason, .. } => {
                Some(format!("continue:{}", reason.as_str()))
            }
            WorkflowControllerDecision::Stop { status } => Some(format!("stop:{status:?}")),
        };

        WorkflowControllerSnapshot {
            workflow_id: self.workflow_id.clone(),
            workflow_root_id: self.workflow_root_id.clone(),
            active_unit_id: self.active_unit_id.clone(),
            child_runs: self.child_runs.clone(),
            graph_closeout_required: self.graph_closeout_required,
            direct_closeout_required: self.direct_closeout_required,
            bootstrap: self.bootstrap.clone(),
            graph_shape: self.graph_shape,
            planning: self.planning.clone(),
            closeout_ready: self.closeout.ready,
            budget: self.budget.clone(),
            counters: self.counters.clone(),
            next_decision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum WorkflowBootstrapState {
    #[default]
    Unspecified,
    Required,
    Complete {
        workflow_root_id: String,
    },
    Skipped {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowGraphShape {
    #[default]
    Unspecified,
    RootOnly,
    NeedsDecomposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum WorkflowPlanningState {
    #[default]
    Unspecified,
    RootOnly,
    AwaitingDecomposition,
    Decomposed {
        child_unit_ids: Vec<String>,
        completed_child_unit_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunBudget {
    pub max_turns: Option<u32>,
    pub max_idle_secs: Option<u64>,
}

impl Default for WorkflowRunBudget {
    fn default() -> Self {
        Self {
            max_turns: Some(200),
            max_idle_secs: Some(30 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunCounters {
    pub started_unix_secs: u64,
    pub last_activity_unix_secs: Option<u64>,
    pub turns: u32,
}

impl Default for WorkflowRunCounters {
    fn default() -> Self {
        Self {
            started_unix_secs: now_unix_secs(),
            last_activity_unix_secs: None,
            turns: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowBudgetStatus {
    WithinBudget,
    Exhausted(String),
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowCloseoutCheck {
    pub ready: bool,
    pub remaining: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowControllerDecision {
    Continue {
        prompt: String,
        reason: ContinueReason,
    },
    Stop {
        status: RunFinalStatus,
    },
}

pub fn workflow_bootstrap_prompt() -> String {
    "Durable workflow bootstrap is required before presenting the work as complete. Create or bind the root workflow work item for this goal, attach acceptance/verification, and continue from the graph; do not close out until the controller has a workflow root or bootstrap is explicitly skipped by runtime policy.".to_string()
}

pub fn workflow_supervision_prompt() -> String {
    "Orchestration has started, so continue supervising it instead of presenting the work as complete. Inspect workflow run_state/logs for active child runs, coordinate ready work, retry or escalate failed units, and only stop when workflow closeout is verified, a concrete blocker exists, or no runnable work remains.".to_string()
}

pub fn workflow_decomposition_prompt() -> String {
    "This workflow was classified as needing decomposition. Create real child workflow tasks for separable durable work products under the workflow root, then execute the first ready child. Do not create lifecycle-only tasks such as verify, closeout, or run tests.".to_string()
}

pub fn workflow_graph_closeout_prompt() -> String {
    "Workflow graph state changed, so do not present the work as complete yet. Inspect the relevant workflow units/tree/next state, verify acceptance and blockers, run required checks, then close or update units before final closeout.".to_string()
}

pub fn workflow_direct_closeout_prompt() -> String {
    "Direct work changed, so do not present the work as complete yet. Inspect the diff, run the narrowest relevant verification, and produce closeout evidence before final closeout.".to_string()
}

pub fn workflow_closeout_prompt() -> String {
    "Workflow execution is not ready for closeout. Review remaining workflow units, child runs, required verification, unresolved decisions, and evidence before presenting the work as complete.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_closeout_prompts_do_not_request_literal_status_headings() {
        let prompts = [
            workflow_bootstrap_prompt(),
            workflow_supervision_prompt(),
            workflow_decomposition_prompt(),
            workflow_graph_closeout_prompt(),
            workflow_direct_closeout_prompt(),
            workflow_closeout_prompt(),
        ];

        for prompt in prompts {
            assert!(!prompt.contains("final DONE"), "prompt was {prompt:?}");
            assert!(!prompt.contains("reporting DONE"), "prompt was {prompt:?}");
            assert!(!prompt.contains("report done"), "prompt was {prompt:?}");
        }
    }

    #[test]
    fn decomposition_requires_real_child_units_before_done() {
        let mut controller = WorkflowRunController::new().with_workflow_root_id("28.1");
        controller.set_graph_shape(WorkflowGraphShape::NeedsDecomposition);

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_decomposition_prompt(),
                reason: ContinueReason::WorkflowDecomposition,
            }
        );
        assert!(!controller.closeout_check().ready);
    }

    #[test]
    fn recording_child_unit_sets_decomposed_planning_and_active_unit() {
        let mut controller = WorkflowRunController::new().with_workflow_root_id("28.1");
        controller.set_graph_shape(WorkflowGraphShape::NeedsDecomposition);
        controller.record_child_unit("28.1.1");

        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1.1"));
        assert_eq!(
            controller.planning,
            WorkflowPlanningState::Decomposed {
                child_unit_ids: vec!["28.1.1".to_string()],
                completed_child_unit_ids: Vec::new(),
            }
        );
        assert_eq!(
            controller.snapshot().planning,
            WorkflowPlanningState::Decomposed {
                child_unit_ids: vec!["28.1.1".to_string()],
                completed_child_unit_ids: Vec::new(),
            }
        );
    }

    #[test]
    fn completing_child_unit_advances_to_next_child_then_root() {
        let mut controller = WorkflowRunController::new().with_workflow_root_id("28.1");
        controller.set_graph_shape(WorkflowGraphShape::NeedsDecomposition);
        controller.record_child_unit("28.1.1");
        controller.record_child_unit("28.1.2");

        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1.1"));
        controller.complete_unit("28.1.1");
        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1.2"));
        assert!(!controller.closeout_check().ready);

        controller.complete_unit("28.1.2");
        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1"));
        assert!(controller.closeout_check().ready);
        assert_eq!(
            controller.planning,
            WorkflowPlanningState::Decomposed {
                child_unit_ids: vec!["28.1.1".to_string(), "28.1.2".to_string()],
                completed_child_unit_ids: vec!["28.1.1".to_string(), "28.1.2".to_string()],
            }
        );
    }

    #[test]
    fn root_only_shape_does_not_require_decomposition() {
        let mut controller = WorkflowRunController::new().with_workflow_root_id("28.1");
        controller.set_graph_shape(WorkflowGraphShape::RootOnly);

        assert_eq!(controller.planning, WorkflowPlanningState::RootOnly);
        assert!(controller.closeout_check().ready);
    }

    #[test]
    fn required_bootstrap_continues_until_workflow_root_bound() {
        let mut controller = WorkflowRunController::new();
        controller.require_bootstrap();

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_bootstrap_prompt(),
                reason: ContinueReason::WorkflowBootstrap,
            }
        );
        assert!(!controller.closeout_check().ready);

        controller.bind_workflow_root("28.1.3.4");
        assert_eq!(controller.workflow_root_id.as_deref(), Some("28.1.3.4"));
        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1.3.4"));
        assert_eq!(
            controller.snapshot().active_unit_id.as_deref(),
            Some("28.1.3.4")
        );
        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Done {
                    reason: StopReason::WorkCompleted,
                },
            }
        );
    }

    #[test]
    fn explicit_active_unit_overrides_root_binding() {
        let controller = WorkflowRunController::new()
            .with_workflow_root_id("28.1")
            .with_active_unit_id("28.1.2");

        assert_eq!(controller.workflow_root_id.as_deref(), Some("28.1"));
        assert_eq!(controller.active_unit_id.as_deref(), Some("28.1.2"));
        assert_eq!(
            controller.snapshot().active_unit_id.as_deref(),
            Some("28.1.2")
        );
    }

    #[test]
    fn skipped_bootstrap_preserves_simple_turns() {
        let mut controller = WorkflowRunController::new();
        controller.skip_bootstrap("explanation-only request");

        assert!(controller.closeout_check().ready);
    }

    #[test]
    fn controller_round_trips_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workflow-controller.json");
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_graph_changed();
        controller.record_workflow_orchestration_started(Some("run-1".into()));

        controller.save_to_path(&path).expect("save controller");
        let loaded = WorkflowRunController::load_from_path(&path).expect("load controller");

        assert_eq!(
            loaded.graph_closeout_required,
            controller.graph_closeout_required
        );
        assert_eq!(loaded.child_runs, controller.child_runs);
    }

    #[test]
    fn turn_budget_exhaustion_blocks_workflow() {
        let mut controller = WorkflowRunController::new();
        controller.budget.max_turns = Some(1);
        controller.tick_turn();

        assert!(matches!(
            controller.budget_status(),
            WorkflowBudgetStatus::Exhausted(_)
        ));
        assert!(matches!(
            controller.decide_next(),
            WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Blocked { .. }
            }
        ));
    }

    #[test]
    fn run_state_status_updates_child_run_terminal_state() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_orchestration_started(Some("run-1".into()));
        controller.update_child_run_status("run-1", WorkflowChildRunStatus::Done);

        assert!(controller.closeout_check().ready);
        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Done {
                    reason: StopReason::WorkCompleted,
                },
            }
        );
    }

    #[test]
    fn workflow_run_status_maps_failures_to_failed_child_run() {
        assert_eq!(
            WorkflowChildRunStatus::from_workflow_run_status("done", Some(1)),
            WorkflowChildRunStatus::Failed
        );
        assert_eq!(
            WorkflowChildRunStatus::from_workflow_run_status("running", None),
            WorkflowChildRunStatus::Running
        );
    }

    #[test]
    fn workflow_orchestration_creates_supervision_obligation() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_orchestration_started(Some("run-1".into()));

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_supervision_prompt(),
                reason: ContinueReason::OrchestrationProgress,
            }
        );
    }

    #[test]
    fn workflow_orchestration_without_concrete_run_id_does_not_create_phantom_child_run() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_orchestration_started(None);
        controller.record_workflow_orchestration_started(Some("unknown".into()));
        controller.record_workflow_orchestration_started(Some("   ".into()));

        assert!(controller.child_runs.is_empty());
        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_graph_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            }
        );
    }

    #[test]
    fn incomplete_closeout_continues_even_without_child_runs() {
        let mut controller = WorkflowRunController::new();
        controller.closeout.ready = false;

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            }
        );
    }

    #[test]
    fn workflow_graph_change_requires_graph_closeout_without_workflow_run() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_graph_changed();

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_graph_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            }
        );
        let check = controller.closeout_check();
        assert!(!check.ready);
        assert!(check
            .remaining
            .iter()
            .any(|item| item.contains("workflow graph changed")));
    }

    #[test]
    fn direct_work_change_requires_direct_closeout_without_mana() {
        let mut controller = WorkflowRunController::new();
        controller.record_direct_work_changed();

        assert_eq!(
            controller.decide_next(),
            WorkflowControllerDecision::Continue {
                prompt: workflow_direct_closeout_prompt(),
                reason: ContinueReason::WorkflowCloseout,
            }
        );
    }

    #[test]
    fn record_closeout_ready_clears_closeout_obligations() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_graph_changed();
        controller.record_direct_work_changed();
        controller.record_closeout_ready();

        assert!(controller.closeout_check().ready);
    }

    #[test]
    fn closeout_downgrades_done_when_child_run_is_still_running() {
        let mut controller = WorkflowRunController::new();
        controller.record_workflow_orchestration_started(Some("run-1".into()));

        let status = controller.enforce_closeout_status(RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        });

        assert!(matches!(status, RunFinalStatus::DoneWithConcerns { .. }));
        if let RunFinalStatus::DoneWithConcerns { concerns, .. } = status {
            assert!(concerns
                .iter()
                .any(|concern| concern.contains("child run run-1 is not terminal")));
        }
    }

    #[test]
    fn closeout_blocks_done_when_child_run_failed() {
        let controller = WorkflowRunController {
            child_runs: vec![WorkflowChildRun {
                run_id: "run-1".into(),
                status: WorkflowChildRunStatus::Failed,
            }],
            ..WorkflowRunController::new()
        };

        let status = controller.enforce_closeout_status(RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        });

        assert!(matches!(
            status,
            RunFinalStatus::Blocked {
                reason: StopReason::ExecutionBlocked,
                ..
            }
        ));
    }

    #[test]
    fn ready_closeout_preserves_done() {
        let controller = WorkflowRunController {
            child_runs: vec![WorkflowChildRun {
                run_id: "run-1".into(),
                status: WorkflowChildRunStatus::Done,
            }],
            ..WorkflowRunController::new()
        };

        let status = controller.enforce_closeout_status(RunFinalStatus::Done {
            reason: StopReason::WorkCompleted,
        });

        assert_eq!(
            status,
            RunFinalStatus::Done {
                reason: StopReason::WorkCompleted,
            }
        );
    }

    #[test]
    fn blocker_stops_as_blocked() {
        let mut controller = WorkflowRunController::new();
        controller.closeout.blocker = Some("decision required".into());

        assert!(matches!(
            controller.decide_next(),
            WorkflowControllerDecision::Stop {
                status: RunFinalStatus::Blocked { .. }
            }
        ));
    }
}
