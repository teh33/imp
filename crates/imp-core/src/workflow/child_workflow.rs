use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::{
    Agent, AgentCommand, AgentEvent, AgentHandle, ParentRunId, RunFinalStatus, SubagentArtifactRef,
    SubagentContext, SubagentEvent, SubagentInput, SubagentMergePolicy, SubagentResourceLimits,
    SubagentRole, SubagentRunId, SubagentStatus,
};
use crate::error::Result;
use crate::roles::{Role, RoleRegistry, RoleRegistryError, RoleToolPolicy};
use crate::workflow::{AutonomyMode, VerificationRequirement, WorkflowContract, WorkflowType};

fn policy_decision(policy: &ChildStalePolicy, reason: String) -> ChildWorkflowPolicyDecision {
    match policy.action {
        ChildStalePolicyAction::NotifyParent => {
            ChildWorkflowPolicyDecision::NotifyParent { reason }
        }
        ChildStalePolicyAction::MarkStale => ChildWorkflowPolicyDecision::MarkStale {
            reason,
            idle_timeout_secs: policy.idle_timeout_secs,
        },
        ChildStalePolicyAction::Cancel => ChildWorkflowPolicyDecision::Cancel { reason },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChildWorkflowId(pub String);

impl ChildWorkflowId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for ChildWorkflowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ParentWorkflowRef {
    pub workflow_id: Option<String>,
    pub run_id: Option<String>,
    pub workflow_unit_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildWorkflowSpec {
    pub id: ChildWorkflowId,
    pub parent: ParentWorkflowRef,
    pub parent_contract: WorkflowContract,
    pub role: String,
    pub title: String,
    pub prompt: String,
    pub contract_ref: Option<PathBuf>,
    pub contract: WorkflowContract,
    pub required: bool,
}

impl Default for ChildWorkflowSpec {
    fn default() -> Self {
        Self {
            id: ChildWorkflowId::new(String::new()),
            parent: ParentWorkflowRef::default(),
            parent_contract: WorkflowContract::default(),
            role: String::new(),
            title: String::new(),
            prompt: String::new(),
            contract_ref: None,
            contract: WorkflowContract::default(),
            required: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ChildWorkflowStatus {
    #[default]
    Planned,
    Queued,
    Starting,
    Running,
    WaitingForApproval,
    WaitingForTool,
    WaitingForParent,
    Blocked,
    Stale,
    Cancelling,
    Cancelled,
    Failed,
    Done,
    DoneWithConcerns,
    Integrated,
}

impl ChildWorkflowStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Failed | Self::Done | Self::DoneWithConcerns | Self::Integrated
        )
    }

    pub fn blocks_required_parent(self) -> bool {
        matches!(
            self,
            Self::Blocked | Self::Stale | Self::Cancelled | Self::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildWorkflowRun {
    pub spec: ChildWorkflowSpec,
    pub status: ChildWorkflowStatus,
    pub summary: Option<ChildWorkflowSummary>,
    pub evidence_refs: Vec<ChildEvidenceRef>,
    pub lifecycle: Vec<ChildWorkflowLifecycleEvent>,
    pub cancellation: Option<ChildCancellation>,
    pub stale: Option<ChildStaleState>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ChildWorkflowRun {
    pub fn new(spec: ChildWorkflowSpec) -> Self {
        let now = Utc::now();
        Self {
            spec,
            status: ChildWorkflowStatus::Planned,
            summary: None,
            evidence_refs: Vec::new(),
            lifecycle: vec![ChildWorkflowLifecycleEvent {
                status: ChildWorkflowStatus::Planned,
                message: Some("child workflow planned".into()),
                at: now,
            }],
            cancellation: None,
            stale: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
        }
    }

    pub fn transition(&mut self, status: ChildWorkflowStatus, message: impl Into<Option<String>>) {
        let now = Utc::now();
        self.status = status;
        self.updated_at = now;
        if status == ChildWorkflowStatus::Running && self.started_at.is_none() {
            self.started_at = Some(now);
        }
        if status.is_terminal() {
            self.completed_at = Some(now);
        }
        self.lifecycle.push(ChildWorkflowLifecycleEvent {
            status,
            message: message.into(),
            at: now,
        });
    }

    pub fn request_cancellation(&mut self, reason: impl Into<String>, by: Option<String>) {
        let now = Utc::now();
        self.cancellation = Some(ChildCancellation {
            reason: reason.into(),
            requested_by: by,
            requested_at: now,
            completed_at: None,
        });
        self.transition(
            ChildWorkflowStatus::Cancelling,
            Some("cancellation requested".into()),
        );
    }

    pub fn mark_stale(&mut self, reason: impl Into<String>, idle_timeout_secs: u64) {
        let now = Utc::now();
        self.stale = Some(ChildStaleState {
            reason: reason.into(),
            idle_timeout_secs,
            last_progress_at: self.updated_at,
            marked_at: now,
        });
        self.transition(
            ChildWorkflowStatus::Stale,
            Some("child workflow is stale".into()),
        );
    }

    pub fn apply_policy_decision(&mut self, decision: ChildWorkflowPolicyDecision) {
        match decision {
            ChildWorkflowPolicyDecision::Continue => {}
            ChildWorkflowPolicyDecision::NotifyParent { reason } => {
                self.lifecycle.push(ChildWorkflowLifecycleEvent {
                    status: self.status,
                    message: Some(format!("notify parent: {reason}")),
                    at: Utc::now(),
                });
            }
            ChildWorkflowPolicyDecision::MarkStale {
                reason,
                idle_timeout_secs,
            } => self.mark_stale(reason, idle_timeout_secs),
            ChildWorkflowPolicyDecision::Cancel { reason } => {
                self.request_cancellation(reason, Some("child-workflow-policy".into()))
            }
        }
    }

    pub fn stale_policy_decision(
        &self,
        policy: &ChildStalePolicy,
        now: DateTime<Utc>,
        health: &ChildWorkflowHealth,
    ) -> ChildWorkflowPolicyDecision {
        if self.status.is_terminal() || matches!(self.status, ChildWorkflowStatus::Cancelling) {
            return ChildWorkflowPolicyDecision::Continue;
        }
        if health.waiting_for_approval {
            return ChildWorkflowPolicyDecision::NotifyParent {
                reason: "child workflow is waiting for approval".into(),
            };
        }
        if health.waiting_for_tool {
            return ChildWorkflowPolicyDecision::NotifyParent {
                reason: "child workflow is waiting for a tool or resource".into(),
            };
        }
        if let Some(limit) = policy.repeated_failure_limit {
            if health.repeated_failures >= limit {
                return policy_decision(
                    policy,
                    format!("child workflow hit repeated failure limit ({limit})"),
                );
            }
        }
        let idle_secs = now
            .signed_duration_since(self.updated_at)
            .num_seconds()
            .max(0) as u64;
        if idle_secs >= policy.idle_timeout_secs {
            return policy_decision(policy, format!("child workflow idle for {idle_secs}s"));
        }
        if health.no_output {
            if let Some(timeout) = policy.no_output_timeout_secs {
                if idle_secs >= timeout {
                    return policy_decision(
                        policy,
                        format!("child workflow produced no output for {idle_secs}s"),
                    );
                }
            }
        }
        ChildWorkflowPolicyDecision::Continue
    }
}

impl Default for ChildWorkflowRun {
    fn default() -> Self {
        Self::new(ChildWorkflowSpec::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChildWorkflowSummary {
    pub status: ChildWorkflowStatus,
    pub summary: String,
    pub findings: Vec<String>,
    pub concerns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChildEvidenceRef {
    pub kind: String,
    pub path: PathBuf,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildWorkflowLifecycleEvent {
    pub status: ChildWorkflowStatus,
    pub message: Option<String>,
    pub at: DateTime<Utc>,
}

impl Default for ChildWorkflowLifecycleEvent {
    fn default() -> Self {
        Self {
            status: ChildWorkflowStatus::Planned,
            message: None,
            at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildCancellation {
    pub reason: String,
    pub requested_by: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Default for ChildCancellation {
    fn default() -> Self {
        Self {
            reason: String::new(),
            requested_by: None,
            requested_at: Utc::now(),
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ChildStalePolicyAction {
    NotifyParent,
    #[default]
    MarkStale,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildStalePolicy {
    pub idle_timeout_secs: u64,
    pub no_output_timeout_secs: Option<u64>,
    pub repeated_failure_limit: Option<u32>,
    pub action: ChildStalePolicyAction,
}

impl Default for ChildStalePolicy {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 300,
            no_output_timeout_secs: None,
            repeated_failure_limit: None,
            action: ChildStalePolicyAction::MarkStale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildWorkflowPolicyDecision {
    Continue,
    NotifyParent {
        reason: String,
    },
    MarkStale {
        reason: String,
        idle_timeout_secs: u64,
    },
    Cancel {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChildWorkflowHealth {
    pub waiting_for_approval: bool,
    pub waiting_for_tool: bool,
    pub no_output: bool,
    pub repeated_failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChildStaleState {
    pub reason: String,
    pub idle_timeout_secs: u64,
    pub last_progress_at: DateTime<Utc>,
    pub marked_at: DateTime<Utc>,
}

impl Default for ChildStaleState {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            reason: String::new(),
            idle_timeout_secs: 0,
            last_progress_at: now,
            marked_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildWorkflowRequest {
    pub id: String,
    pub role: String,
    pub title: String,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub struct ChildWorkflowPlan {
    pub id: String,
    pub role: Role,
    pub contract: WorkflowContract,
    pub evidence_handoff: ChildEvidenceHandoff,
}

#[derive(Debug, Clone)]
pub struct ChildWorkflowExecutionResult {
    pub run: ChildWorkflowRun,
    pub final_status: Option<RunFinalStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSubagentSpawn {
    pub input: SubagentInput,
    pub started_event: SubagentEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSubagentCompletion {
    pub status: SubagentStatus,
    pub summary: String,
    pub event: SubagentEvent,
}

pub fn workflow_subagent_input(
    workflow_id: &str,
    step_id: &str,
    action: &crate::workflow::WorkflowStepAction,
) -> SubagentInput {
    let child_run_id = SubagentRunId::new(format!("workflow-{workflow_id}-{step_id}"));
    let role = subagent_role(action.role.as_deref(), action.worker.as_deref());
    SubagentInput {
        parent_run_id: ParentRunId::new(format!("workflow-{workflow_id}")),
        child_run_id,
        role,
        objective: action.objective.clone(),
        context: SubagentContext {
            instructions: action.instructions.clone(),
            messages: vec![format!(
                "Workflow `{workflow_id}` delegated step `{step_id}`. Report progress, blockers, evidence, and final outcome through the workflow communication mailbox."
            )],
            files: Vec::new(),
            artifacts: action
                .completion
                .artifacts
                .iter()
                .map(|path| SubagentArtifactRef {
                    name: path.display().to_string(),
                    path: Some(path.clone()),
                    description: Some("required completion artifact".to_string()),
                })
                .collect(),
        },
        resource_limits: SubagentResourceLimits {
            writable_paths: action.write_scope.clone(),
            allowed_paths: action.write_scope.clone(),
            ..SubagentResourceLimits::default()
        },
        merge_policy: merge_policy_for_role(action.role.as_deref()),
        output_contract: Some(format!(
            "Complete workflow `{workflow_id}` step `{step_id}`. Required checks: {}. Required artifacts: {}.",
            action.completion.checks.join(", "),
            action
                .completion
                .artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn workflow_subagent_spawn(
    workflow_id: &str,
    step_id: &str,
    action: &crate::workflow::WorkflowStepAction,
) -> WorkflowSubagentSpawn {
    let input = workflow_subagent_input(workflow_id, step_id, action);
    let started_event = SubagentEvent::Started {
        child_run_id: input.child_run_id.clone(),
        role: input.role.clone(),
        objective: input.objective.clone(),
    };
    WorkflowSubagentSpawn {
        input,
        started_event,
    }
}

pub fn workflow_subagent_completion(
    input: &SubagentInput,
    final_status: Option<&RunFinalStatus>,
) -> WorkflowSubagentCompletion {
    let status = subagent_status_from_final_status(final_status);
    let summary = match final_status {
        Some(RunFinalStatus::Done { reason }) => {
            format!("subagent completed: {}", reason.as_str())
        }
        Some(RunFinalStatus::DoneWithConcerns { concerns, .. }) => {
            format!("subagent completed with concerns: {}", concerns.join("; "))
        }
        Some(RunFinalStatus::Blocked { message, .. }) => format!("subagent blocked: {message}"),
        Some(RunFinalStatus::NeedsUserInput { question }) => {
            format!("subagent needs user input: {question}")
        }
        Some(RunFinalStatus::Failed { message }) => format!("subagent failed: {message}"),
        Some(RunFinalStatus::Cancelled) => "subagent cancelled".to_string(),
        None => "subagent ended without a final status".to_string(),
    };
    let event = SubagentEvent::Completed {
        outcome: crate::agent::SubagentOutcome {
            child_run_id: input.child_run_id.clone(),
            role: input.role.clone(),
            status: status.clone(),
            summary: summary.clone(),
            evidence: input.context.artifacts.clone(),
            files_changed: input.resource_limits.writable_paths.clone(),
            files_inspected: input.resource_limits.allowed_paths.clone(),
            verification_results: Vec::new(),
            blockers: match status {
                SubagentStatus::Blocked => vec![summary.clone()],
                _ => Vec::new(),
            },
            follow_ups: Vec::new(),
            diagnostics: Vec::new(),
            confidence: None,
        },
    };
    WorkflowSubagentCompletion {
        status,
        summary,
        event,
    }
}

fn subagent_role(role: Option<&str>, worker: Option<&str>) -> SubagentRole {
    match role.or(worker).unwrap_or("worker") {
        "searcher" | "researcher" => SubagentRole::Searcher,
        "planner" => SubagentRole::Planner,
        "coder" | "builder" | "implementer" => SubagentRole::Implementer,
        "verifier" => SubagentRole::Verifier,
        "reviewer" => SubagentRole::Reviewer,
        "synthesizer" => SubagentRole::Synthesizer,
        other => SubagentRole::Custom(other.to_string()),
    }
}

fn merge_policy_for_role(role: Option<&str>) -> SubagentMergePolicy {
    match role.unwrap_or("worker") {
        "verifier" => SubagentMergePolicy::Verify,
        "reviewer" => SubagentMergePolicy::Review,
        "synthesizer" => SubagentMergePolicy::Synthesize,
        "coder" | "builder" | "implementer" => SubagentMergePolicy::Apply,
        other => SubagentMergePolicy::Custom(other.to_string()),
    }
}

fn subagent_status_from_final_status(status: Option<&RunFinalStatus>) -> SubagentStatus {
    match status {
        Some(RunFinalStatus::Done { .. }) => SubagentStatus::Success,
        Some(RunFinalStatus::DoneWithConcerns { .. }) => SubagentStatus::Incomplete,
        Some(RunFinalStatus::Blocked { .. }) | Some(RunFinalStatus::NeedsUserInput { .. }) => {
            SubagentStatus::Blocked
        }
        Some(RunFinalStatus::Cancelled) => SubagentStatus::Cancelled,
        Some(RunFinalStatus::Failed { .. }) | None => SubagentStatus::Failed,
    }
}

pub struct ChildWorkflowRunner {
    pub run: ChildWorkflowRun,
    pub agent: Agent,
    pub handle: AgentHandle,
}

impl ChildWorkflowRunner {
    pub fn new(run: ChildWorkflowRun, agent: Agent, handle: AgentHandle) -> Self {
        Self { run, agent, handle }
    }

    pub async fn run(mut self) -> Result<ChildWorkflowExecutionResult> {
        self.run.transition(
            ChildWorkflowStatus::Starting,
            Some("starting child agent".into()),
        );
        let prompt = self.run.spec.prompt.clone();
        self.run.transition(
            ChildWorkflowStatus::Running,
            Some("child agent running".into()),
        );
        let mut event_rx = self.handle.event_rx;
        let agent_result = self.agent.run(prompt);
        tokio::pin!(agent_result);
        let mut final_status = None;
        let mut run_result = None;
        loop {
            tokio::select! {
                result = &mut agent_result, if run_result.is_none() => {
                    run_result = Some(result);
                }
                maybe_event = event_rx.recv(), if final_status.is_none() => {
                    match maybe_event {
                        Some(AgentEvent::AgentEnd { status, .. }) => {
                            final_status = Some(status);
                        }
                        Some(_) => {}
                        None => {}
                    }
                }
                else => break,
            }
            if run_result.is_some() && final_status.is_some() {
                break;
            }
            if run_result.is_some() && event_rx.is_closed() {
                break;
            }
        }
        let result = run_result.unwrap_or_else(|| Ok(()));
        if result.is_err() {
            self.run.transition(
                ChildWorkflowStatus::Failed,
                Some("child agent failed".into()),
            );
        } else {
            self.run.transition(
                child_status_from_final_status(final_status.as_ref()),
                Some("child agent completed".into()),
            );
        }
        result?;
        Ok(ChildWorkflowExecutionResult {
            run: self.run,
            final_status,
        })
    }

    pub async fn cancel(&self) {
        let _ = self.handle.command_tx.send(AgentCommand::Cancel).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChildEvidenceHandoff {
    pub role: String,
    pub required_evidence: Vec<String>,
    pub output_schema: Option<String>,
    pub output_required_sections: Vec<String>,
}

#[derive(Debug)]
pub enum ChildWorkflowError {
    RoleRegistry(RoleRegistryError),
    UnknownRole(String),
    RoleNotDelegable(String),
}

impl std::fmt::Display for ChildWorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoleRegistry(err) => write!(f, "role registry error: {err}"),
            Self::UnknownRole(role) => write!(f, "unknown child workflow role `{role}`"),
            Self::RoleNotDelegable(role) => write!(
                f,
                "role `{role}` is not eligible for child workflow delegation"
            ),
        }
    }
}

impl std::error::Error for ChildWorkflowError {}

impl From<RoleRegistryError> for ChildWorkflowError {
    fn from(value: RoleRegistryError) -> Self {
        Self::RoleRegistry(value)
    }
}

pub fn create_child_workflow_contract(
    parent: &WorkflowContract,
    role: &Role,
    id: impl Into<String>,
    title: impl Into<String>,
    prompt: impl Into<String>,
) -> WorkflowContract {
    let mut contract = WorkflowContract {
        id: Some(id.into()),
        title: Some(title.into()),
        objective: format!(
            "Parent objective: {}\n\nChild task: {}",
            parent.objective,
            prompt.into()
        ),
        workflow_type: workflow_type_for_role(role).unwrap_or(parent.workflow_type),
        risk_level: parent.risk_level,
        autonomy_mode: autonomy_for_role(parent.autonomy_mode, role),
        workspace_scope: parent.workspace_scope.clone(),
        trust_scope: parent.trust_scope.clone(),
        parent_workflow_ref: parent.id.clone(),
        workflow_unit_ref: parent.workflow_unit_ref.clone(),
        role: Some(role.name.clone()),
        ..WorkflowContract::default()
    };
    contract.tool_permissions = parent.tool_permissions.clone();
    apply_role_tool_permissions(&mut contract, role);
    contract.approval_requirements = parent.approval_requirements.clone();
    contract.required_verification = parent.required_verification.clone();
    for command in &role.verification.suggested_commands {
        contract
            .required_verification
            .push(VerificationRequirement::command(command.clone()));
    }
    contract.closeout_criteria = parent.closeout_criteria.clone();
    apply_role_closeout_criteria(&mut contract, role);
    contract
}

pub fn plan_child_workflow(
    registry: &RoleRegistry,
    request: ChildWorkflowRequest,
) -> std::result::Result<ChildWorkflowPlan, ChildWorkflowError> {
    let role = registry
        .resolve(&request.role)
        .ok_or_else(|| ChildWorkflowError::UnknownRole(request.role.clone()))?;
    if !role.child_workflow.eligible {
        return Err(ChildWorkflowError::RoleNotDelegable(request.role));
    }

    let parent = WorkflowContract {
        id: Some(format!("parent-for-{}", request.id)),
        objective: "Delegate child workflow".into(),
        ..WorkflowContract::default()
    };
    let contract = create_child_workflow_contract(
        &parent,
        &role,
        request.id.clone(),
        request.title,
        request.prompt,
    );

    let evidence_handoff = ChildEvidenceHandoff {
        role: role.name.clone(),
        required_evidence: role
            .required_evidence
            .iter()
            .filter(|evidence| evidence.required)
            .map(|evidence| evidence.kind.clone())
            .collect(),
        output_schema: role
            .output_schema
            .as_ref()
            .map(|schema| schema.name.clone()),
        output_required_sections: role
            .output_schema
            .as_ref()
            .map(|schema| schema.required_sections.clone())
            .unwrap_or_default(),
    };

    Ok(ChildWorkflowPlan {
        id: request.id,
        role,
        contract,
        evidence_handoff,
    })
}

fn apply_role_tool_permissions(contract: &mut WorkflowContract, role: &Role) {
    match &role.tool_policy {
        RoleToolPolicy::All => {}
        RoleToolPolicy::Only(tools) => {
            contract.tool_permissions.allowed_tools.clear();
            contract
                .tool_permissions
                .allowed_tools
                .extend(tools.iter().cloned());
        }
        RoleToolPolicy::AllExcept(tools) => {
            contract
                .tool_permissions
                .denied_tools
                .extend(tools.iter().cloned());
        }
    }
}

fn apply_role_closeout_criteria(contract: &mut WorkflowContract, role: &Role) {
    for evidence in &role.required_evidence {
        let label = if evidence.description.is_empty() {
            evidence.kind.clone()
        } else {
            format!("{}: {}", evidence.kind, evidence.description)
        };
        if evidence.required {
            contract
                .closeout_criteria
                .criteria
                .push(format!("required evidence: {label}"));
        } else {
            contract
                .closeout_criteria
                .criteria
                .push(format!("optional evidence: {label}"));
        }
    }
    if let Some(schema) = &role.output_schema {
        if let Some(contract_text) = &schema.output_contract {
            contract
                .closeout_criteria
                .criteria
                .push(format!("role output contract: {contract_text}"));
        }
    }
}

fn autonomy_for_role(parent: AutonomyMode, role: &Role) -> AutonomyMode {
    if !role.autonomy.can_modify_files {
        return AutonomyMode::Safe;
    }
    parent
}

fn workflow_type_for_role(role: &Role) -> Option<WorkflowType> {
    match role.name.as_str() {
        "verifier" => Some(WorkflowType::Verification),
        "reviewer" => Some(WorkflowType::Review),
        "researcher" => Some(WorkflowType::Investigation),
        "planner" => Some(WorkflowType::Planning),
        "coder" => Some(WorkflowType::CodeChange),
        "integrator" => Some(WorkflowType::Orchestration),
        _ => None,
    }
}

fn child_status_from_final_status(status: Option<&RunFinalStatus>) -> ChildWorkflowStatus {
    match status {
        Some(RunFinalStatus::Done { .. }) => ChildWorkflowStatus::Done,
        Some(RunFinalStatus::DoneWithConcerns { .. }) => ChildWorkflowStatus::DoneWithConcerns,
        Some(RunFinalStatus::Blocked { .. }) | Some(RunFinalStatus::NeedsUserInput { .. }) => {
            ChildWorkflowStatus::Blocked
        }
        Some(RunFinalStatus::Cancelled) => ChildWorkflowStatus::Cancelled,
        Some(RunFinalStatus::Failed { .. }) | None => ChildWorkflowStatus::Failed,
    }
}

#[cfg(test)]
#[path = "child_workflow/tests.rs"]
mod tests;
