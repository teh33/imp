use super::{RunFinalStatus, StopReason};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod evidence;
mod projection;

pub fn enforce_task_closeout(proposed: RunFinalStatus, state: &SessionTaskState) -> RunFinalStatus {
    let issues = state.closeout_issues();
    if issues.is_empty() {
        return proposed;
    }
    match proposed {
        RunFinalStatus::Done { .. } | RunFinalStatus::DoneWithConcerns { .. } => {
            RunFinalStatus::Blocked {
                reason: StopReason::CloseoutIncomplete,
                message: issues.join("; "),
            }
        }
        other => other,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStepStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStep {
    pub id: u32,
    pub description: String,
    pub status: TaskStepStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckRecord {
    pub command: String,
    pub passed: bool,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTaskState {
    pub objective: String,
    pub steps: Vec<TaskStep>,
    pub constraints: Vec<String>,
    pub changed_paths: BTreeSet<String>,
    pub checks: Vec<CheckRecord>,
    pub failures: Vec<String>,
    pub blockers: Vec<String>,
    pub verification_required: bool,
    #[serde(skip)]
    enabled: bool,
    #[serde(skip)]
    planning_active: bool,
    next_step_id: u32,
}

impl Default for SessionTaskState {
    fn default() -> Self {
        Self {
            objective: String::new(),
            steps: Vec::new(),
            constraints: Vec::new(),
            changed_paths: BTreeSet::new(),
            checks: Vec::new(),
            failures: Vec::new(),
            blockers: Vec::new(),
            verification_required: false,
            enabled: false,
            planning_active: false,
            next_step_id: 1,
        }
    }
}

impl SessionTaskState {
    pub fn new(objective: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            steps: Vec::new(),
            constraints: Vec::new(),
            changed_paths: BTreeSet::new(),
            checks: Vec::new(),
            failures: Vec::new(),
            blockers: Vec::new(),
            verification_required: false,
            enabled: true,
            planning_active: false,
            next_step_id: 1,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        *self = Self::default();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn planning_active(&self) -> bool {
        self.enabled && self.planning_active
    }

    pub fn activate_planning(&mut self) {
        if self.enabled {
            self.planning_active = true;
        }
    }

    pub fn should_project(&self) -> bool {
        self.planning_active()
            || !self.steps.is_empty()
            || !self.constraints.is_empty()
            || !self.changed_paths.is_empty()
            || !self.unresolved_failures().is_empty()
            || !self.blockers.is_empty()
            || self.verification_required
    }

    pub fn begin_prompt(&mut self, objective: impl Into<String>) {
        if !self.enabled {
            return;
        }
        let objective = objective.into();
        if !self.objective.trim().is_empty() && !self.closeout_issues().is_empty() {
            self.planning_active = true;
            return;
        }
        *self = Self::new(&objective);
        if prompt_requests_planning(&objective) {
            self.planning_active = true;
        }
    }

    pub fn reset(&mut self, objective: impl Into<String>) {
        if !self.enabled {
            return;
        }
        *self = Self::new(objective);
    }

    pub fn replace_plan(&mut self, descriptions: Vec<String>) -> Result<(), String> {
        self.activate_planning();
        if descriptions.is_empty() {
            return Err("steps must not be empty".into());
        }
        if descriptions.len() > 30 {
            return Err("steps must contain at most 30 items".into());
        }
        let steps = descriptions
            .into_iter()
            .map(|description| self.new_step(description))
            .collect::<Result<Vec<_>, _>>()?;
        self.steps = steps;
        Ok(())
    }

    pub fn update_step(
        &mut self,
        id: u32,
        status: TaskStepStatus,
        note: Option<String>,
    ) -> Result<(), String> {
        let step = self
            .steps
            .iter_mut()
            .find(|step| step.id == id)
            .ok_or_else(|| format!("unknown task step id: {id}"))?;
        step.status = status;
        step.note = normalize_optional(note);
        Ok(())
    }

    pub fn add_constraint(&mut self, constraint: String) -> Result<(), String> {
        let constraint = require_text(constraint, "constraint")?;
        if !self.constraints.contains(&constraint) {
            self.constraints.push(constraint);
        }
        Ok(())
    }

    pub fn add_blocker(&mut self, blocker: String) -> Result<(), String> {
        self.activate_planning();
        let blocker = require_text(blocker, "blocker")?;
        if !self.blockers.contains(&blocker) {
            self.blockers.push(blocker);
        }
        Ok(())
    }

    pub fn resolve_blocker(&mut self, blocker: &str) -> Result<(), String> {
        let before = self.blockers.len();
        self.blockers.retain(|existing| existing != blocker);
        if self.blockers.len() == before {
            return Err(format!("unknown blocker: {blocker}"));
        }
        Ok(())
    }

    pub fn closeout_issues(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut issues = Vec::new();
        if self.verification_required {
            issues.push("verification is required after file changes".to_string());
        }
        issues.extend(self.unresolved_failures());
        issues.extend(
            self.blockers
                .iter()
                .map(|blocker| format!("unresolved blocker: {blocker}")),
        );
        issues.extend(self.steps.iter().filter_map(|step| {
            (!matches!(step.status, TaskStepStatus::Completed)).then(|| {
                format!(
                    "step {} is {}: {}",
                    step.id,
                    projection::status_name(step.status),
                    step.description
                )
            })
        }));
        issues
    }

    fn new_step(&mut self, description: String) -> Result<TaskStep, String> {
        let description = require_text(description, "step")?;
        let id = self.next_step_id;
        self.next_step_id = self.next_step_id.saturating_add(1);
        Ok(TaskStep {
            id,
            description,
            status: TaskStepStatus::Pending,
            note: None,
        })
    }
}

fn prompt_requests_planning(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    [
        "make a plan",
        "create a plan",
        "write a plan",
        "plan this",
        "plan the",
        "track the work",
        "track this work",
        "long-running",
        "multi-step",
        "multiple phases",
        "break this down",
        "break down the",
    ]
    .iter()
    .any(|phrase| prompt.contains(phrase))
}

fn require_text(value: String, field: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(format!("{field} must not be empty"))
    } else if value.len() > 500 {
        Err(format!("{field} must contain at most 500 characters"))
    } else {
        Ok(value)
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

#[cfg(test)]
mod tests;
