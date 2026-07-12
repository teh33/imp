use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Runtime-local identifier for a bounded subagent execution.
///
/// This is scoped to the parent imp run. It is intentionally not a durable work
/// item id, lease id, or scheduler id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubagentRunId(pub String);

impl SubagentRunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Runtime-local identifier for the parent execution that spawned a subagent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParentRunId(pub String);

impl ParentRunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Role a bounded subagent should play within the parent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentRole {
    Searcher,
    Planner,
    Implementer,
    Verifier,
    Reviewer,
    Synthesizer,
    Custom(String),
}

/// Context a parent run gives to a bounded subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentContext {
    pub instructions: Vec<String>,
    pub messages: Vec<String>,
    pub files: Vec<SubagentFileContext>,
    pub artifacts: Vec<SubagentArtifactRef>,
}

/// A file or path-scoped context reference for a bounded subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentFileContext {
    pub path: PathBuf,
    pub note: Option<String>,
    pub read_only: bool,
}

/// Runtime artifact reference produced or consumed within the parent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentArtifactRef {
    pub name: String,
    pub path: Option<PathBuf>,
    pub description: Option<String>,
}

/// Resource limits for a bounded subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentResourceLimits {
    pub timeout_seconds: Option<u64>,
    pub max_model_tokens: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub max_parallel_children: Option<u32>,
    pub allowed_paths: Vec<PathBuf>,
    pub writable_paths: Vec<PathBuf>,
}

/// How the parent run should consume a bounded subagent outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubagentMergePolicy {
    #[default]
    Inform,
    Verify,
    Review,
    Apply,
    Synthesize,
    Escalate,
    Custom(String),
}

/// Input packet for a bounded subagent execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInput {
    pub parent_run_id: ParentRunId,
    pub child_run_id: SubagentRunId,
    #[serde(default)]
    pub model: Option<String>,
    pub role: SubagentRole,
    pub objective: String,
    pub context: SubagentContext,
    pub resource_limits: SubagentResourceLimits,
    pub merge_policy: SubagentMergePolicy,
    pub output_contract: Option<String>,
}

/// Runtime status for a bounded subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Pending,
    Running,
    Success,
    Incomplete,
    Blocked,
    Failed,
    Cancelled,
}

impl SubagentStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Success | Self::Incomplete | Self::Blocked | Self::Failed | Self::Cancelled
        )
    }
}

/// Structured result returned by a bounded subagent to its parent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentOutcome {
    pub child_run_id: SubagentRunId,
    pub role: SubagentRole,
    pub status: SubagentStatus,
    pub summary: String,
    pub evidence: Vec<SubagentArtifactRef>,
    pub files_changed: Vec<PathBuf>,
    pub files_inspected: Vec<PathBuf>,
    pub verification_results: Vec<String>,
    pub blockers: Vec<String>,
    pub follow_ups: Vec<String>,
    pub diagnostics: Vec<String>,
    pub confidence: Option<SubagentConfidence>,
}

/// Coarse confidence/risk label for a bounded subagent outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentConfidence {
    Low,
    Medium,
    High,
}

/// Runtime event emitted while a bounded subagent executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentEvent {
    Started {
        child_run_id: SubagentRunId,
        role: SubagentRole,
        objective: String,
    },
    Message {
        child_run_id: SubagentRunId,
        text: String,
    },
    ToolStarted {
        child_run_id: SubagentRunId,
        tool_name: String,
    },
    ToolCompleted {
        child_run_id: SubagentRunId,
        tool_name: String,
        success: bool,
    },
    Artifact {
        child_run_id: SubagentRunId,
        artifact: SubagentArtifactRef,
    },
    Blocked {
        child_run_id: SubagentRunId,
        reason: String,
    },
    Cancelled {
        child_run_id: SubagentRunId,
        reason: Option<String>,
    },
    Completed {
        outcome: SubagentOutcome,
    },
    Failed {
        child_run_id: SubagentRunId,
        error: String,
    },
    Merged {
        child_run_id: SubagentRunId,
        policy: SubagentMergePolicy,
    },
}

/// Planned bounded subagent execution that has not been spawned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentPlan {
    pub input: SubagentInput,
}

/// Result of requesting a bounded subagent spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSpawnResult {
    pub child_run_id: SubagentRunId,
    pub events: Vec<SubagentEvent>,
}

/// Result of requesting cancellation for a bounded subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentCancelResult {
    pub child_run_id: SubagentRunId,
    pub event: SubagentEvent,
}

/// Result of merging a bounded subagent outcome into the parent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentMergeResult {
    pub child_run_id: SubagentRunId,
    pub policy: SubagentMergePolicy,
    pub event: SubagentEvent,
}

/// Error returned by a bounded subagent coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentCoordinatorError {
    Unsupported { operation: String },
    InvalidInput { reason: String },
    NotFound { child_run_id: SubagentRunId },
    Failed { reason: String },
}

impl SubagentCoordinatorError {
    pub fn unsupported(operation: impl Into<String>) -> Self {
        Self::Unsupported {
            operation: operation.into(),
        }
    }
}

/// Runtime-local coordinator for bounded subagents.
///
/// This is intentionally scoped to imp runtime orchestration. Implementations
/// must not assume durable task ids, leases, board status, or scheduler state.
pub trait SubagentCoordinator {
    fn plan(&self, input: SubagentInput) -> Result<SubagentPlan, SubagentCoordinatorError>;

    fn spawn(&self, plan: SubagentPlan) -> Result<SubagentSpawnResult, SubagentCoordinatorError>;

    fn cancel(
        &self,
        child_run_id: &SubagentRunId,
        reason: Option<String>,
    ) -> Result<SubagentCancelResult, SubagentCoordinatorError>;

    fn merge(
        &self,
        outcome: SubagentOutcome,
        policy: SubagentMergePolicy,
    ) -> Result<SubagentMergeResult, SubagentCoordinatorError>;
}

/// Default coordinator used until a real bounded subagent executor is enabled.
///
/// Planning is allowed because it has no side effects. Execution operations
/// return explicit unsupported errors instead of silently succeeding.
#[derive(Debug, Clone, Default)]
pub struct NoopSubagentCoordinator;

impl SubagentCoordinator for NoopSubagentCoordinator {
    fn plan(&self, input: SubagentInput) -> Result<SubagentPlan, SubagentCoordinatorError> {
        Ok(SubagentPlan { input })
    }

    fn spawn(&self, _plan: SubagentPlan) -> Result<SubagentSpawnResult, SubagentCoordinatorError> {
        Err(SubagentCoordinatorError::unsupported("spawn"))
    }

    fn cancel(
        &self,
        _child_run_id: &SubagentRunId,
        _reason: Option<String>,
    ) -> Result<SubagentCancelResult, SubagentCoordinatorError> {
        Err(SubagentCoordinatorError::unsupported("cancel"))
    }

    fn merge(
        &self,
        _outcome: SubagentOutcome,
        _policy: SubagentMergePolicy,
    ) -> Result<SubagentMergeResult, SubagentCoordinatorError> {
        Err(SubagentCoordinatorError::unsupported("merge"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_status_terminal_states_are_explicit() {
        assert!(!SubagentStatus::Pending.is_terminal());
        assert!(!SubagentStatus::Running.is_terminal());
        assert!(SubagentStatus::Success.is_terminal());
        assert!(SubagentStatus::Incomplete.is_terminal());
        assert!(SubagentStatus::Blocked.is_terminal());
        assert!(SubagentStatus::Failed.is_terminal());
        assert!(SubagentStatus::Cancelled.is_terminal());
    }

    fn sample_subagent_input() -> SubagentInput {
        SubagentInput {
            parent_run_id: ParentRunId::new("parent-1"),
            child_run_id: SubagentRunId::new("child-1"),
            model: None,
            role: SubagentRole::Verifier,
            objective: "Check the patch".to_string(),
            context: SubagentContext::default(),
            resource_limits: SubagentResourceLimits::default(),
            merge_policy: SubagentMergePolicy::Verify,
            output_contract: None,
        }
    }

    #[test]
    fn noop_coordinator_plans_without_execution() {
        let coordinator = NoopSubagentCoordinator;
        let input = sample_subagent_input();

        let plan = coordinator.plan(input.clone()).expect("plan subagent");
        assert_eq!(plan.input, input);
    }

    #[test]
    fn noop_coordinator_rejects_execution_operations_explicitly() {
        let coordinator = NoopSubagentCoordinator;
        let input = sample_subagent_input();
        let plan = coordinator.plan(input.clone()).expect("plan subagent");

        assert_eq!(
            coordinator.spawn(plan),
            Err(SubagentCoordinatorError::Unsupported {
                operation: "spawn".to_string()
            })
        );
        assert_eq!(
            coordinator.cancel(&input.child_run_id, Some("stop".to_string())),
            Err(SubagentCoordinatorError::Unsupported {
                operation: "cancel".to_string()
            })
        );

        let outcome = SubagentOutcome {
            child_run_id: input.child_run_id,
            role: input.role,
            status: SubagentStatus::Success,
            summary: "verified".to_string(),
            evidence: Vec::new(),
            files_changed: Vec::new(),
            files_inspected: Vec::new(),
            verification_results: Vec::new(),
            blockers: Vec::new(),
            follow_ups: Vec::new(),
            diagnostics: Vec::new(),
            confidence: Some(SubagentConfidence::High),
        };
        assert_eq!(
            coordinator.merge(outcome, SubagentMergePolicy::Verify),
            Err(SubagentCoordinatorError::Unsupported {
                operation: "merge".to_string()
            })
        );
    }

    #[test]
    fn subagent_event_uses_runtime_scoped_ids() {
        let event = SubagentEvent::Started {
            child_run_id: SubagentRunId::new("child-1"),
            role: SubagentRole::Verifier,
            objective: "Check the patch".to_string(),
        };

        let json = serde_json::to_string(&event).expect("serialize event");
        assert!(json.contains("child-1"));
        assert!(!json.contains("task_id"));
        assert!(!json.contains("lease"));
        assert!(!json.contains("board"));
    }
}
