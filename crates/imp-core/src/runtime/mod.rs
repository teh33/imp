use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::{BrowserEvent, BrowserEventKind, RunFinalStatus};
use crate::workflow::{
    AutonomyMode, ChildWorkflowRun, ChildWorkflowStatus, VerificationGate, WorkspaceScope,
    WorktreeCloseoutResult, WorktreeRunMetadata,
};

pub const RUNTIME_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp_ms: Option<u64>,
    pub kind: RuntimeEventKind,
}

impl Default for RuntimeEvent {
    fn default() -> Self {
        Self {
            schema_version: RUNTIME_SCHEMA_VERSION,
            run_id: String::new(),
            sequence: 0,
            timestamp_ms: None,
            kind: RuntimeEventKind::Unknown {
                name: String::new(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RuntimeEventKind {
    AgentStarted {
        model: String,
    },
    AgentEnded {
        status: RuntimeFinalStatus,
        usage: Option<RuntimeUsageSummary>,
    },
    TurnStarted {
        index: u32,
    },
    TurnAssessed {
        index: u32,
        summary: Option<String>,
    },
    TurnEnded {
        index: u32,
    },
    MessageStarted {
        role: String,
        summary: Option<String>,
    },
    MessageDelta {
        delta: String,
    },
    MessageEnded {
        role: String,
        summary: Option<String>,
    },
    ToolStarted {
        tool_call: RuntimeToolCall,
    },
    ToolOutput {
        tool_call_id: String,
        output_delta: String,
    },
    ToolCompleted {
        tool_call: RuntimeToolCall,
    },
    BrowserUpdated {
        event: BrowserEvent,
    },
    ApprovalPending {
        approval: RuntimeApprovalRef,
    },
    ApprovalResolved {
        approval: RuntimeApprovalRef,
    },
    PolicyDecision {
        decision: RuntimePolicyDecision,
    },
    WorkflowControllerUpdated {
        snapshot: crate::workflow::WorkflowControllerSnapshot,
    },
    VerificationUpdated {
        gate: VerificationGate,
    },
    EvidenceUpdated {
        artifact: RuntimeArtifactRef,
    },
    ChildWorkflowUpdated {
        child: RuntimeChildWorkflowSummary,
    },
    WorktreeUpdated {
        worktree: RuntimeWorktreeState,
    },
    ManaUpdated {
        workflow_ref: RuntimeManaRef,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
    Timing {
        stage: String,
        duration_ms: Option<u64>,
        success: Option<bool>,
    },
    RecoveryCheckpoint {
        kind: String,
        turn: u32,
        tool_call_id: Option<String>,
    },
    Unknown {
        name: String,
    },
}

impl Default for RuntimeEventKind {
    fn default() -> Self {
        Self::Unknown {
            name: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    pub workflow: RuntimeWorkflowSummary,
    pub autonomy_mode: Option<AutonomyMode>,
    pub workspace: RuntimeWorkspaceState,
    pub phase: RuntimePhase,
    pub active_tools: Vec<RuntimeToolCall>,
    pub completed_tools: Vec<RuntimeToolCall>,
    pub pending_approvals: Vec<RuntimeApprovalRef>,
    pub policy_decisions: Vec<RuntimePolicyDecision>,
    pub verification_gates: Vec<VerificationGate>,
    pub evidence_refs: Vec<RuntimeArtifactRef>,
    pub child_workflows: Vec<RuntimeChildWorkflowSummary>,
    pub final_status: Option<RuntimeFinalStatus>,
    pub workflow_refs: Vec<RuntimeManaRef>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub status_items: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateAccumulator {
    snapshot: RuntimeStateSnapshot,
}

impl RuntimeStateAccumulator {
    pub fn new(run_id: impl Into<String>) -> Self {
        let mut snapshot = RuntimeStateSnapshot::default();
        snapshot.workflow.run_id = Some(run_id.into());
        Self { snapshot }
    }

    pub fn from_snapshot(snapshot: RuntimeStateSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn apply(&mut self, event: &RuntimeEvent) {
        if self.snapshot.workflow.run_id.is_none() && !event.run_id.is_empty() {
            self.snapshot.workflow.run_id = Some(event.run_id.clone());
        }
        match &event.kind {
            RuntimeEventKind::AgentStarted { model } => {
                self.snapshot.phase = RuntimePhase::Running;
                self.snapshot.workflow.model = Some(model.clone());
                self.snapshot
                    .status_items
                    .insert("model".into(), model.clone());
                self.snapshot
                    .status_items
                    .insert("phase".into(), "running".into());
            }
            RuntimeEventKind::AgentEnded { status, usage } => {
                self.snapshot.final_status = Some(status.clone());
                self.snapshot.phase = runtime_phase_for_final_status(status);
                self.snapshot.status_items.insert(
                    "phase".into(),
                    match self.snapshot.phase {
                        RuntimePhase::Completed => "completed",
                        RuntimePhase::Blocked => "blocked",
                        RuntimePhase::Failed => "failed",
                        _ => "ended",
                    }
                    .into(),
                );
                if let Some(usage) = usage {
                    self.snapshot
                        .status_items
                        .insert("tokens".into(), usage.total_tokens.to_string());
                    if let Some(cost) = &usage.total_cost {
                        self.snapshot
                            .status_items
                            .insert("cost".into(), cost.clone());
                    }
                }
            }
            RuntimeEventKind::TurnStarted { index } => {
                self.snapshot.phase = RuntimePhase::Running;
                self.snapshot
                    .status_items
                    .insert("turn".into(), index.to_string());
            }
            RuntimeEventKind::TurnAssessed { index, summary } => {
                self.snapshot.status_items.insert(
                    "turn-assessment".into(),
                    summary
                        .clone()
                        .unwrap_or_else(|| format!("turn {index} assessed")),
                );
            }
            RuntimeEventKind::TurnEnded { index } => {
                self.snapshot
                    .status_items
                    .insert("last-turn".into(), index.to_string());
            }
            RuntimeEventKind::MessageStarted { role, summary }
            | RuntimeEventKind::MessageEnded { role, summary } => {
                self.snapshot
                    .status_items
                    .insert("message-role".into(), role.clone());
                if let Some(summary) = summary {
                    self.snapshot
                        .status_items
                        .insert("message".into(), summary.clone());
                }
            }
            RuntimeEventKind::MessageDelta { delta } => {
                if !delta.is_empty() {
                    self.snapshot
                        .status_items
                        .insert("message-delta".into(), truncate_runtime_text(delta, 120));
                }
            }
            RuntimeEventKind::ToolStarted { tool_call } => {
                self.snapshot.phase = RuntimePhase::WaitingForTool;
                upsert_tool(&mut self.snapshot.active_tools, tool_call.clone());
                self.snapshot
                    .status_items
                    .insert("tool".into(), tool_call.name.clone());
            }
            RuntimeEventKind::ToolOutput {
                tool_call_id,
                output_delta,
            } => {
                if let Some(tool) = self
                    .snapshot
                    .active_tools
                    .iter_mut()
                    .find(|tool| tool.id == *tool_call_id)
                {
                    tool.output_preview = Some(match &tool.output_preview {
                        Some(existing) if !existing.is_empty() => {
                            truncate_runtime_text(&format!("{existing}{output_delta}"), 400)
                        }
                        _ => truncate_runtime_text(output_delta, 400),
                    });
                }
            }
            RuntimeEventKind::ToolCompleted { tool_call } => {
                self.snapshot
                    .active_tools
                    .retain(|tool| tool.id != tool_call.id);
                upsert_tool(&mut self.snapshot.completed_tools, tool_call.clone());
                self.snapshot.phase = RuntimePhase::Running;
                self.snapshot.status_items.insert(
                    "last-tool".into(),
                    format!("{}:{:?}", tool_call.name, tool_call.status),
                );
            }
            RuntimeEventKind::BrowserUpdated { event } => {
                self.apply_browser_event(event);
            }
            RuntimeEventKind::ApprovalPending { approval } => {
                self.snapshot.phase = RuntimePhase::WaitingForApproval;
                upsert_approval(&mut self.snapshot.pending_approvals, approval.clone());
            }
            RuntimeEventKind::ApprovalResolved { approval } => {
                self.snapshot
                    .pending_approvals
                    .retain(|pending| pending.id != approval.id);
                self.snapshot.status_items.insert(
                    "last-approval".into(),
                    format!("{}:{:?}", approval.summary, approval.status),
                );
            }
            RuntimeEventKind::PolicyDecision { decision } => {
                self.snapshot.policy_decisions.push(decision.clone());
                if matches!(decision.decision, RuntimePolicyDecisionKind::Deny) {
                    self.snapshot.phase = RuntimePhase::Blocked;
                }
            }
            RuntimeEventKind::WorkflowControllerUpdated { snapshot } => {
                self.snapshot.workflow.controller = Some(snapshot.clone());
                if let Some(next) = &snapshot.next_decision {
                    self.snapshot
                        .status_items
                        .insert("workflow_next".into(), next.clone());
                }
            }
            RuntimeEventKind::VerificationUpdated { gate } => {
                self.snapshot.phase = RuntimePhase::Verifying;
                upsert_verification_gate(&mut self.snapshot.verification_gates, gate.clone());
                self.snapshot
                    .status_items
                    .insert("verification".into(), gate.name.clone());
            }
            RuntimeEventKind::EvidenceUpdated { artifact } => {
                upsert_artifact(&mut self.snapshot.evidence_refs, artifact.clone());
                self.snapshot
                    .status_items
                    .insert("evidence".into(), artifact.path.display().to_string());
            }
            RuntimeEventKind::ChildWorkflowUpdated { child } => {
                upsert_child_workflow(&mut self.snapshot.child_workflows, child.clone());
                self.snapshot.status_items.insert(
                    "child-workflow".into(),
                    format!("{}:{:?}", child.id, child.status),
                );
                self.snapshot.phase = runtime_phase_for_child_status(child.status);
                for artifact in &child.evidence_refs {
                    upsert_artifact(&mut self.snapshot.evidence_refs, artifact.clone());
                }
            }
            RuntimeEventKind::WorktreeUpdated { worktree } => {
                self.snapshot.workspace.scope = WorkspaceScope::Worktree {
                    path: worktree.metadata.worktree_path.clone(),
                    branch: Some(worktree.metadata.branch.clone()),
                };
                self.snapshot.workspace.worktree = Some(worktree.clone());
                self.snapshot.status_items.insert(
                    "worktree".into(),
                    format!(
                        "{} @ {}",
                        worktree.metadata.branch,
                        worktree.metadata.worktree_path.display()
                    ),
                );
                if !worktree.metadata.patch_path.as_os_str().is_empty() {
                    self.snapshot.status_items.insert(
                        "worktree-diff".into(),
                        worktree.metadata.patch_path.display().to_string(),
                    );
                }
                if let Some(closeout) = &worktree.closeout {
                    self.snapshot
                        .status_items
                        .insert("worktree-closeout".into(), closeout.message.clone());
                }
            }
            RuntimeEventKind::ManaUpdated { workflow_ref } => {
                upsert_workflow_ref(&mut self.snapshot.workflow_refs, workflow_ref.clone());
            }
            RuntimeEventKind::Warning { message } => {
                self.snapshot.warnings.push(message.clone());
            }
            RuntimeEventKind::Error { message } => {
                self.snapshot.errors.push(message.clone());
                self.snapshot.phase = RuntimePhase::Failed;
            }
            RuntimeEventKind::Timing { stage, .. } => {
                self.snapshot
                    .status_items
                    .insert("timing".into(), stage.clone());
            }
            RuntimeEventKind::RecoveryCheckpoint {
                kind,
                turn,
                tool_call_id,
            } => {
                self.snapshot.status_items.insert(
                    "recovery".into(),
                    match tool_call_id {
                        Some(tool_call_id) => format!("{kind}: turn {turn}, tool {tool_call_id}"),
                        None => format!("{kind}: turn {turn}"),
                    },
                );
            }
            RuntimeEventKind::Unknown { name } => {
                self.snapshot
                    .status_items
                    .insert("last-unknown-event".into(), name.clone());
            }
        }
    }

    fn apply_browser_event(&mut self, event: &BrowserEvent) {
        self.snapshot.status_items.insert(
            "browser".into(),
            format!(
                "{:?}:{}",
                event.kind,
                event.domain.as_deref().unwrap_or("local")
            ),
        );
        let approval = browser_approval_ref(event);
        match event.kind {
            BrowserEventKind::InputRequested => {
                self.snapshot.phase = RuntimePhase::WaitingForApproval;
                upsert_approval(&mut self.snapshot.pending_approvals, approval);
            }
            BrowserEventKind::InputApproved | BrowserEventKind::InputDenied => {
                self.snapshot
                    .pending_approvals
                    .retain(|pending| pending.id != approval.id);
                self.snapshot.status_items.insert(
                    "last-approval".into(),
                    format!("{}:{:?}", approval.summary, approval.status),
                );
                self.snapshot.phase = RuntimePhase::Running;
            }
            _ => {}
        }
    }

    pub fn snapshot(&self) -> RuntimeStateSnapshot {
        self.snapshot.clone()
    }
}

fn browser_approval_ref(event: &BrowserEvent) -> RuntimeApprovalRef {
    let session = event.session_id.as_deref().unwrap_or("browser");
    let action = event.action.as_deref().unwrap_or("input");
    RuntimeApprovalRef {
        id: format!("{session}:{action}"),
        summary: format!(
            "browser {action} on {}",
            event.domain.as_deref().unwrap_or("current page")
        ),
        status: match event.kind {
            BrowserEventKind::InputApproved => RuntimeApprovalStatus::Approved,
            BrowserEventKind::InputDenied => RuntimeApprovalStatus::Denied,
            _ => RuntimeApprovalStatus::Pending,
        },
        requested_by: Some("browser".into()),
    }
}

fn runtime_phase_for_final_status(status: &RuntimeFinalStatus) -> RuntimePhase {
    match status {
        RuntimeFinalStatus::Done | RuntimeFinalStatus::DoneWithConcerns { .. } => {
            RuntimePhase::Completed
        }
        RuntimeFinalStatus::Blocked { .. } | RuntimeFinalStatus::NeedsContext { .. } => {
            RuntimePhase::Blocked
        }
        RuntimeFinalStatus::Failed { .. } => RuntimePhase::Failed,
    }
}

fn runtime_phase_for_child_status(status: ChildWorkflowStatus) -> RuntimePhase {
    match status {
        ChildWorkflowStatus::Planned
        | ChildWorkflowStatus::Queued
        | ChildWorkflowStatus::Starting => RuntimePhase::Starting,
        ChildWorkflowStatus::Running => RuntimePhase::Running,
        ChildWorkflowStatus::WaitingForApproval => RuntimePhase::WaitingForApproval,
        ChildWorkflowStatus::WaitingForTool => RuntimePhase::WaitingForTool,
        ChildWorkflowStatus::WaitingForParent
        | ChildWorkflowStatus::Blocked
        | ChildWorkflowStatus::Stale => RuntimePhase::Blocked,
        ChildWorkflowStatus::Cancelling
        | ChildWorkflowStatus::Cancelled
        | ChildWorkflowStatus::Failed => RuntimePhase::Failed,
        ChildWorkflowStatus::Done
        | ChildWorkflowStatus::DoneWithConcerns
        | ChildWorkflowStatus::Integrated => RuntimePhase::Completed,
    }
}

fn upsert_tool(tools: &mut Vec<RuntimeToolCall>, tool_call: RuntimeToolCall) {
    if let Some(existing) = tools.iter_mut().find(|tool| tool.id == tool_call.id) {
        *existing = tool_call;
    } else {
        tools.push(tool_call);
    }
}

fn upsert_approval(approvals: &mut Vec<RuntimeApprovalRef>, approval: RuntimeApprovalRef) {
    if let Some(existing) = approvals.iter_mut().find(|item| item.id == approval.id) {
        *existing = approval;
    } else {
        approvals.push(approval);
    }
}

fn upsert_artifact(artifacts: &mut Vec<RuntimeArtifactRef>, artifact: RuntimeArtifactRef) {
    if let Some(existing) = artifacts
        .iter_mut()
        .find(|item| item.kind == artifact.kind && item.path == artifact.path)
    {
        *existing = artifact;
    } else {
        artifacts.push(artifact);
    }
}

fn upsert_child_workflow(
    children: &mut Vec<RuntimeChildWorkflowSummary>,
    child: RuntimeChildWorkflowSummary,
) {
    if let Some(existing) = children.iter_mut().find(|item| item.id == child.id) {
        *existing = child;
    } else {
        children.push(child);
    }
}

fn upsert_workflow_ref(workflow_refs: &mut Vec<RuntimeManaRef>, workflow_ref: RuntimeManaRef) {
    if let Some(existing) = workflow_refs
        .iter_mut()
        .find(|item| item.id == workflow_ref.id)
    {
        *existing = workflow_ref;
    } else {
        workflow_refs.push(workflow_ref);
    }
}

fn upsert_verification_gate(gates: &mut Vec<VerificationGate>, gate: VerificationGate) {
    if let Some(existing) = gates.iter_mut().find(|item| item.name == gate.name) {
        *existing = gate;
    } else {
        gates.push(gate);
    }
}

fn truncate_runtime_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

impl Default for RuntimeStateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: RUNTIME_SCHEMA_VERSION,
            workflow: RuntimeWorkflowSummary::default(),
            autonomy_mode: None,
            workspace: RuntimeWorkspaceState::default(),
            phase: RuntimePhase::Idle,
            active_tools: Vec::new(),
            completed_tools: Vec::new(),
            pending_approvals: Vec::new(),
            policy_decisions: Vec::new(),
            verification_gates: Vec::new(),
            evidence_refs: Vec::new(),
            child_workflows: Vec::new(),
            final_status: None,
            workflow_refs: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            status_items: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeWorkflowSummary {
    pub run_id: Option<String>,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub contract_summary: Option<String>,
    pub model: Option<String>,
    pub controller: Option<crate::workflow::WorkflowControllerSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeWorkspaceState {
    pub cwd: Option<PathBuf>,
    pub scope: WorkspaceScope,
    pub worktree: Option<RuntimeWorktreeState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePhase {
    #[default]
    Idle,
    Starting,
    Running,
    WaitingForTool,
    WaitingForApproval,
    Verifying,
    Completed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeUsageSummary {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub raw_total_tokens: u32,
    pub effective_total_tokens: u32,
    pub total_tokens: u32,
    pub total_cost: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeToolCall {
    pub id: String,
    pub name: String,
    pub status: RuntimeToolStatus,
    pub summary: Option<String>,
    pub args_preview: Option<String>,
    pub output_preview: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeToolStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeApprovalRef {
    pub id: String,
    pub summary: String,
    pub status: RuntimeApprovalStatus,
    pub requested_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeApprovalStatus {
    #[default]
    Pending,
    Approved,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimePolicyDecision {
    pub id: Option<String>,
    pub subject: String,
    pub decision: RuntimePolicyDecisionKind,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePolicyDecisionKind {
    Allow,
    #[default]
    Warn,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeArtifactRef {
    pub kind: String,
    pub path: PathBuf,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeChildWorkflowSummary {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub title: Option<String>,
    pub status: ChildWorkflowStatus,
    pub summary: Option<String>,
    pub evidence_refs: Vec<RuntimeArtifactRef>,
    pub concerns: Vec<String>,
    pub last_progress_ms: Option<u64>,
}

impl RuntimeChildWorkflowSummary {
    pub fn from_child_run(run: &ChildWorkflowRun) -> Self {
        Self {
            id: run.spec.id.to_string(),
            parent_id: run.spec.parent.workflow_id.clone(),
            role: run.spec.role.clone(),
            title: Some(run.spec.title.clone()),
            status: run.status,
            summary: run.summary.as_ref().map(|summary| summary.summary.clone()),
            evidence_refs: run
                .evidence_refs
                .iter()
                .map(|evidence| RuntimeArtifactRef {
                    kind: evidence.kind.clone(),
                    path: evidence.path.clone(),
                    summary: evidence.summary.clone(),
                })
                .collect(),
            concerns: run
                .summary
                .as_ref()
                .map(|summary| summary.concerns.clone())
                .unwrap_or_default(),
            last_progress_ms: Some(run.updated_at.timestamp_millis().max(0) as u64),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeManaRef {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeWorktreeState {
    pub metadata: WorktreeRunMetadata,
    pub metadata_path: Option<PathBuf>,
    pub closeout: Option<WorktreeCloseoutResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum RuntimeFinalStatus {
    Done,
    DoneWithConcerns { concerns: Vec<String> },
    Blocked { reason: String },
    NeedsContext { question: String },
    Failed { error: String },
}

impl From<RunFinalStatus> for RuntimeFinalStatus {
    fn from(status: RunFinalStatus) -> Self {
        match status {
            RunFinalStatus::Done { .. } => Self::Done,
            RunFinalStatus::DoneWithConcerns { concerns, .. } => {
                Self::DoneWithConcerns { concerns }
            }
            RunFinalStatus::Blocked { message, .. } => Self::Blocked { reason: message },
            RunFinalStatus::NeedsUserInput { question } => Self::NeedsContext { question },
            RunFinalStatus::Cancelled => Self::Failed {
                error: "cancelled".into(),
            },
            RunFinalStatus::Failed { message } => Self::Failed { error: message },
        }
    }
}

#[cfg(test)]
mod runtime_events;
