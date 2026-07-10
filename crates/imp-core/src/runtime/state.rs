use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::RunFinalStatus;
use crate::workflow::{
    AutonomyMode, ChildWorkflowRun, ChildWorkflowStatus, VerificationCloseoutEffect,
    VerificationGate, WorkspaceScope, WorktreeCloseoutResult, WorktreeRunMetadata,
};

use super::RUNTIME_SCHEMA_VERSION;

pub const MAX_RUNTIME_TOOL_OUTPUT_CHARS: usize = 16_384;
pub const MAX_RUNTIME_WARNING_COUNT: usize = 128;
pub const MAX_RUNTIME_ERROR_COUNT: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    pub revision: u64,
    pub last_sequence: Option<u64>,
    pub workflow: RuntimeWorkflowSummary,
    pub autonomy_mode: Option<AutonomyMode>,
    pub workspace: RuntimeWorkspaceState,
    pub phase: RuntimePhase,
    pub turns: Vec<RuntimeTurn>,
    pub transcript: Vec<RuntimeTranscriptMessage>,
    pub active_message_id: Option<String>,
    pub active_tools: Vec<RuntimeToolCall>,
    pub completed_tools: Vec<RuntimeToolCall>,
    pub pending_approvals: Vec<RuntimeApprovalRef>,
    pub resolved_approvals: Vec<RuntimeApprovalRef>,
    pub policy_decisions: Vec<RuntimePolicyDecision>,
    pub verification_gates: Vec<VerificationGate>,
    pub verification: Vec<RuntimeVerificationUpdate>,
    pub evidence_refs: Vec<RuntimeArtifactRef>,
    pub child_workflows: Vec<RuntimeChildWorkflowSummary>,
    pub usage: RuntimeUsageSummary,
    pub context_usage: RuntimeContextUsage,
    pub final_status: Option<RuntimeFinalStatus>,
    pub workflow_refs: Vec<RuntimeManaRef>,
    pub recovery: Vec<RuntimeRecoverySummary>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub status_items: BTreeMap<String, String>,
}

impl Default for RuntimeStateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: RUNTIME_SCHEMA_VERSION,
            revision: 0,
            last_sequence: None,
            workflow: RuntimeWorkflowSummary::default(),
            autonomy_mode: None,
            workspace: RuntimeWorkspaceState::default(),
            phase: RuntimePhase::Idle,
            turns: Vec::new(),
            transcript: Vec::new(),
            active_message_id: None,
            active_tools: Vec::new(),
            completed_tools: Vec::new(),
            pending_approvals: Vec::new(),
            resolved_approvals: Vec::new(),
            policy_decisions: Vec::new(),
            verification_gates: Vec::new(),
            verification: Vec::new(),
            evidence_refs: Vec::new(),
            child_workflows: Vec::new(),
            usage: RuntimeUsageSummary::default(),
            context_usage: RuntimeContextUsage::default(),
            final_status: None,
            workflow_refs: Vec::new(),
            recovery: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            status_items: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeStateDelta {
    pub revision: u64,
    pub sequence: u64,
    pub phase_changed: bool,
    pub transcript_changed: bool,
    pub tools_changed: bool,
    pub approvals_changed: bool,
    pub policy_changed: bool,
    pub verification_changed: bool,
    pub evidence_changed: bool,
    pub workflow_changed: bool,
    pub usage_changed: bool,
    pub diagnostics_changed: bool,
    pub final_status_changed: bool,
    pub changed_message_id: Option<String>,
    pub changed_tool_call_id: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTurnStatus {
    #[default]
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeTurn {
    pub index: u32,
    pub status: RuntimeTurnStatus,
    pub assessment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMessageRole {
    User,
    #[default]
    Assistant,
    ToolResult,
    Compaction,
    System,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RuntimeAssistantBlock {
    VisibleText { text: String },
    Thinking { text: String },
    ToolCall { tool_call_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeTranscriptMessage {
    pub id: String,
    pub role: RuntimeMessageRole,
    pub blocks: Vec<RuntimeAssistantBlock>,
    pub is_streaming: bool,
    pub timestamp_ms: Option<u64>,
    pub error_replacement: bool,
}

impl RuntimeTranscriptMessage {
    pub fn visible_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match block {
                RuntimeAssistantBlock::VisibleText { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn thinking_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match block {
                RuntimeAssistantBlock::Thinking { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
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
    pub input_cost_micros: u64,
    pub output_cost_micros: u64,
    pub cache_read_cost_micros: u64,
    pub cache_write_cost_micros: u64,
    pub total_cost_micros: u64,
    pub total_cost: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeContextUsage {
    pub used: u32,
    pub display_window: u32,
    pub input_limit: u32,
    pub system_tokens: u32,
    pub tool_definition_tokens: u32,
    pub message_tokens: u32,
    pub output_tokens: u32,
    pub observed_input_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeToolCall {
    pub id: String,
    pub name: String,
    pub status: RuntimeToolStatus,
    pub summary: Option<String>,
    pub args_preview: Option<String>,
    pub arguments: Option<Value>,
    pub output_preview: Option<String>,
    pub details: Option<Value>,
    pub warning: Option<String>,
    pub is_error: bool,
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
    pub warning: Option<String>,
    pub warning_kind: Option<RuntimePolicyWarningKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePolicyWarningKind {
    Trust,
    Extension,
    Policy,
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
pub struct RuntimeVerificationUpdate {
    pub gate: VerificationGate,
    pub closeout_effect: Option<VerificationCloseoutEffect>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWorktreeNoticeKind {
    Created,
    DiffCaptured,
    Closeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeWorktreeNotice {
    pub kind: RuntimeWorktreeNoticeKind,
    pub message: String,
    pub worktree: RuntimeWorktreeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RuntimeRecoverySummary {
    pub kind: String,
    pub turn: u32,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub success: Option<bool>,
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "status")]
pub enum RuntimeFinalStatus {
    Done,
    DoneWithConcerns { concerns: Vec<String> },
    Blocked { reason: String },
    NeedsContext { question: String },
    Cancelled,
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
            RunFinalStatus::Cancelled => Self::Cancelled,
            RunFinalStatus::Failed { message } => Self::Failed { error: message },
        }
    }
}
