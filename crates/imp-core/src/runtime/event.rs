use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::{
    RuntimeApprovalRef, RuntimeArtifactRef, RuntimeChildWorkflowSummary, RuntimeContextUsage,
    RuntimeFinalStatus, RuntimePolicyDecision, RuntimeRecoverySummary, RuntimeToolCall,
    RuntimeTranscriptMessage, RuntimeUsageSummary, RuntimeVerificationUpdate,
    RuntimeWorktreeNotice, RuntimeWorktreeState,
};
use crate::agent::BrowserEvent;
use crate::workflow::{VerificationGate, WorkflowControllerSnapshot};

pub const RUNTIME_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
            kind: RuntimeEventKind::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct RuntimeEventWire {
    schema_version: u32,
    run_id: String,
    sequence: u64,
    timestamp_ms: Option<u64>,
    kind: Value,
}

impl Default for RuntimeEventWire {
    fn default() -> Self {
        Self {
            schema_version: RUNTIME_SCHEMA_VERSION,
            run_id: String::new(),
            sequence: 0,
            timestamp_ms: None,
            kind: Value::Null,
        }
    }
}

impl<'de> Deserialize<'de> for RuntimeEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RuntimeEventWire::deserialize(deserializer)?;
        let kind = serde_json::from_value(wire.kind.clone()).unwrap_or_else(|_| {
            RuntimeEventKind::Unknown {
                name: wire
                    .kind
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            }
        });
        Ok(Self {
            schema_version: wire.schema_version,
            run_id: wire.run_id,
            sequence: wire.sequence,
            timestamp_ms: wire.timestamp_ms,
            kind,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RuntimeAssistantDelta {
    VisibleText { text: String },
    Thinking { text: String },
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
    TurnCompleted {
        index: u32,
        usage: Option<RuntimeUsageSummary>,
    },
    MessageStarted {
        role: String,
        summary: Option<String>,
    },
    MessageObserved {
        message: RuntimeTranscriptMessage,
    },
    MessageDelta {
        delta: String,
    },
    AssistantDelta {
        delta: RuntimeAssistantDelta,
    },
    MessageEnded {
        role: String,
        summary: Option<String>,
    },
    MessageFinalized {
        message: RuntimeTranscriptMessage,
    },
    SessionHydrated {
        transcript: Vec<RuntimeTranscriptMessage>,
        completed_tools: Vec<RuntimeToolCall>,
    },
    ToolDeclared {
        tool_call: RuntimeToolCall,
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
        snapshot: WorkflowControllerSnapshot,
    },
    VerificationUpdated {
        gate: VerificationGate,
    },
    VerificationCompleted {
        update: RuntimeVerificationUpdate,
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
    WorktreeNotice {
        notice: RuntimeWorktreeNotice,
    },
    ManaUpdated {
        workflow_ref: super::RuntimeManaRef,
    },
    ContextUsageUpdated {
        usage: RuntimeContextUsage,
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
    RecoveryUpdated {
        recovery: RuntimeRecoverySummary,
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
