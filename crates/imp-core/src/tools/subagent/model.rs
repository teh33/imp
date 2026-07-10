use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent::{
    SubagentArtifactRef, SubagentConfidence, SubagentInput, SubagentOutcome, SubagentStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SubagentMapping {
    pub version: u32,
    pub parent_run_id: String,
    pub child_run_id: String,
    pub loopr_run_id: String,
    pub loopr_thread_id: String,
    pub loopr_session_id: Option<String>,
    pub loopr_turn_id: Option<String>,
    pub status: SubagentStatus,
    pub role: crate::agent::SubagentRole,
    pub objective: String,
    pub allowed_paths: Vec<PathBuf>,
    pub writable_paths: Vec<PathBuf>,
    pub timeout_seconds: Option<u64>,
    pub artifacts: Vec<SubagentArtifactRef>,
    pub result: Option<String>,
    pub unsupported_resource_limits: Vec<String>,
}

impl SubagentMapping {
    pub(super) fn from_launch(input: &SubagentInput, run_id: String, thread: &LooprThread) -> Self {
        Self {
            version: 1,
            parent_run_id: input.parent_run_id.as_str().to_string(),
            child_run_id: input.child_run_id.as_str().to_string(),
            loopr_run_id: run_id,
            loopr_thread_id: thread.id.clone(),
            loopr_session_id: thread.session_id.clone(),
            loopr_turn_id: thread.turn_id.clone(),
            status: map_loopr_status(&thread.status, None),
            role: input.role.clone(),
            objective: input.objective.clone(),
            allowed_paths: input.resource_limits.allowed_paths.clone(),
            writable_paths: input.resource_limits.writable_paths.clone(),
            timeout_seconds: input.resource_limits.timeout_seconds,
            artifacts: thread.artifacts(),
            result: None,
            unsupported_resource_limits: unsupported_limits(input),
        }
    }

    pub(super) fn outcome(
        &self,
        status: SubagentStatus,
        result: Option<String>,
        child_status: Option<LooprChildStatus>,
        diagnostics: Vec<String>,
    ) -> SubagentOutcome {
        let child_status = child_status.unwrap_or_default();
        let summary = child_status.summary.or(result).unwrap_or_else(|| {
            format!(
                "loopr child {} is {}",
                self.loopr_thread_id,
                status_name(&status)
            )
        });
        SubagentOutcome {
            child_run_id: crate::agent::SubagentRunId::new(self.child_run_id.clone()),
            role: self.role.clone(),
            status,
            summary,
            evidence: self.artifacts.clone(),
            files_changed: child_status.files_changed,
            files_inspected: child_status.files_inspected,
            verification_results: child_status.checks_run,
            blockers: child_status.blockers,
            follow_ups: child_status.recommended_next_prompt.into_iter().collect(),
            diagnostics,
            confidence: child_status.confidence,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct LooprRun {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct LooprThread {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub result_path: Option<PathBuf>,
    #[serde(default)]
    pub status_path: Option<PathBuf>,
    #[serde(default)]
    pub stdout_path: Option<PathBuf>,
    #[serde(default)]
    pub stderr_path: Option<PathBuf>,
}

impl LooprThread {
    pub(super) fn artifacts(&self) -> Vec<SubagentArtifactRef> {
        [
            ("result", &self.result_path),
            ("status", &self.status_path),
            ("stdout", &self.stdout_path),
            ("stderr", &self.stderr_path),
        ]
        .into_iter()
        .filter_map(|(name, path)| {
            path.clone().map(|path| SubagentArtifactRef {
                name: format!("loopr {name}"),
                path: Some(path),
                description: Some(format!("loopr child {name} artifact")),
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct LooprChildStatus {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files_changed: Vec<PathBuf>,
    #[serde(default)]
    pub files_inspected: Vec<PathBuf>,
    #[serde(default, alias = "checks")]
    pub checks_run: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub recommended_next_prompt: Option<String>,
    #[serde(default)]
    pub confidence: Option<SubagentConfidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LooprPoll {
    pub status: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub child_status: Option<LooprChildStatus>,
    #[serde(default)]
    pub child_status_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LooprSent {
    pub thread: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
    pub sent: bool,
}

fn unsupported_limits(input: &SubagentInput) -> Vec<String> {
    let limits = &input.resource_limits;
    [
        limits
            .timeout_seconds
            .map(|value| format!("timeout_seconds={value}")),
        limits
            .max_model_tokens
            .map(|value| format!("max_model_tokens={value}")),
        limits
            .max_tool_calls
            .map(|value| format!("max_tool_calls={value}")),
        limits
            .max_parallel_children
            .map(|value| format!("max_parallel_children={value}")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) fn map_loopr_status(
    status: &str,
    child_status: Option<&LooprChildStatus>,
) -> SubagentStatus {
    match status {
        "created" => SubagentStatus::Pending,
        "running" => SubagentStatus::Running,
        "result_ready" | "joined" => child_status
            .and_then(|s| s.status.as_deref())
            .map(map_child_status)
            .unwrap_or(SubagentStatus::Incomplete),
        "stopped" | "cancelled" => SubagentStatus::Cancelled,
        "failed" => SubagentStatus::Failed,
        _ => SubagentStatus::Incomplete,
    }
}

fn map_child_status(status: &str) -> SubagentStatus {
    match status.to_ascii_lowercase().as_str() {
        "done" | "completed" | "success" | "successful" => SubagentStatus::Success,
        "blocked" => SubagentStatus::Blocked,
        "needs_input" | "needs-input" | "incomplete" => SubagentStatus::Incomplete,
        "failed" | "error" => SubagentStatus::Failed,
        "cancelled" | "canceled" | "stopped" => SubagentStatus::Cancelled,
        "created" => SubagentStatus::Pending,
        _ => SubagentStatus::Running,
    }
}

pub(super) fn status_name(status: &SubagentStatus) -> &'static str {
    match status {
        SubagentStatus::Pending => "pending",
        SubagentStatus::Running => "running",
        SubagentStatus::Success => "success",
        SubagentStatus::Incomplete => "incomplete",
        SubagentStatus::Blocked => "blocked",
        SubagentStatus::Failed => "failed",
        SubagentStatus::Cancelled => "cancelled",
    }
}
