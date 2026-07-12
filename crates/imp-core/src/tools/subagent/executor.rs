use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::contract::{child_environment, child_prompt, contract_args, unsupported_limits};

use crate::agent::{
    SubagentArtifactRef, SubagentInput, SubagentOutcome, SubagentRunId, SubagentStatus,
};
use crate::error::{Error, Result};
use crate::tools::ToolContext;

#[derive(Clone)]
pub(super) struct ImpSubagentExecutor {
    executable: PathBuf,
    child_args: Vec<String>,
}
impl ImpSubagentExecutor {
    pub(super) fn from_current_executable() -> Self {
        Self {
            executable: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("imp")),
            child_args: Vec::new(),
        }
    }
    pub(super) fn launch(
        &self,
        input: &SubagentInput,
        ctx: &ToolContext,
    ) -> Result<imp_subagent::Record> {
        let mut child_args = self.child_args.clone();
        child_args.extend(contract_args(input, ctx));
        let metadata = json!({"role":input.role,"objective":input.objective,"allowed_paths":input.resource_limits.allowed_paths,"writable_paths":input.resource_limits.writable_paths,"output_contract":input.output_contract,"merge_policy":input.merge_policy,"unsupported_resource_limits":unsupported_limits(input)});
        imp_subagent::Executor::new(&ctx.cwd)
            .launch(imp_subagent::LaunchRequest {
                parent_id: input.parent_run_id.as_str().into(),
                child_id: input.child_run_id.as_str().into(),
                model: input
                    .model
                    .clone()
                    .ok_or_else(|| Error::Tool("subagent model was not resolved".into()))?,
                cwd: ctx.cwd.clone(),
                prompt: child_prompt(input),
                executable: self.executable.clone(),
                worker_executable: self.executable.clone(),
                child_args: child_args.clone(),
                environment: child_environment(input, &child_args),
                timeout_seconds: input.resource_limits.timeout_seconds,
                metadata,
            })
            .map_err(convert)
    }
    pub(super) fn status(&self, cwd: &Path, child: &SubagentRunId) -> Result<imp_subagent::Record> {
        let parent = parent_id(cwd, child)?;
        self.executor(cwd)
            .status(&parent, child.as_str())
            .map_err(convert)
    }
    pub(super) fn wait(
        &self,
        cwd: &Path,
        child: &SubagentRunId,
        timeout_seconds: u64,
    ) -> Result<imp_subagent::Record> {
        let parent = parent_id(cwd, child)?;
        self.executor(cwd)
            .wait(
                &parent,
                child.as_str(),
                Duration::from_secs(timeout_seconds),
            )
            .map_err(convert)
    }
    pub(super) fn send(
        &self,
        cwd: &Path,
        child: &SubagentRunId,
        message: String,
    ) -> Result<imp_subagent::Record> {
        let parent = parent_id(cwd, child)?;
        self.executor(cwd)
            .send(&parent, child.as_str(), message)
            .map_err(convert)
    }
    pub(super) fn cancel(&self, cwd: &Path, child: &SubagentRunId) -> Result<imp_subagent::Record> {
        let parent = parent_id(cwd, child)?;
        self.executor(cwd)
            .cancel(&parent, child.as_str())
            .map_err(convert)
    }
    pub(super) fn outcome(&self, record: &imp_subagent::Record) -> SubagentOutcome {
        let status = map_status(&record.status);
        let metadata = &record.metadata;
        SubagentOutcome {
            child_run_id: SubagentRunId::new(&record.child_id),
            role: serde_json::from_value(metadata["role"].clone())
                .unwrap_or(crate::agent::SubagentRole::Custom("unknown".into())),
            status: status.clone(),
            summary: record.summary.clone().unwrap_or_else(|| {
                format!(
                    "subagent {} is {}",
                    record.child_id,
                    status_name(&record.status)
                )
            }),
            evidence: vec![
                artifact("subagent transcript", &record.artifacts.transcript),
                artifact("subagent stderr", &record.artifacts.stderr),
                artifact("subagent session", &record.artifacts.session),
            ],
            files_changed: Vec::new(),
            files_inspected: metadata["allowed_paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| serde_json::from_value(value.clone()).ok())
                .collect(),
            verification_results: Vec::new(),
            blockers: if matches!(status, SubagentStatus::Blocked | SubagentStatus::Incomplete) {
                record.diagnostics.clone()
            } else {
                Vec::new()
            },
            follow_ups: Vec::new(),
            diagnostics: record
                .diagnostics
                .iter()
                .cloned()
                .chain(
                    metadata["unsupported_resource_limits"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str())
                        .map(|limit| format!("not enforced by imp-subagent: {limit}")),
                )
                .collect(),
            confidence: None,
        }
    }
    fn executor(&self, cwd: &Path) -> imp_subagent::Executor {
        imp_subagent::Executor::new(cwd)
    }
}
fn artifact(name: &str, path: &Path) -> SubagentArtifactRef {
    SubagentArtifactRef {
        name: name.into(),
        path: Some(path.into()),
        description: Some("imp-subagent artifact".into()),
    }
}
fn parent_id(cwd: &Path, child: &SubagentRunId) -> Result<String> {
    let root = cwd.join(".imp/runs");
    let entries = std::fs::read_dir(&root).map_err(|_| {
        Error::Tool(format!(
            "stale or unknown subagent child id `{}`",
            child.as_str()
        ))
    })?;
    let mut parent = None;
    for entry in entries.flatten() {
        let state = entry
            .path()
            .join("subagents")
            .join(child.as_str())
            .join("state.json");
        if state.is_file()
            && parent
                .replace(entry.file_name().to_string_lossy().into_owned())
                .is_some()
        {
            return Err(Error::Tool(format!(
                "ambiguous subagent child id `{}`",
                child.as_str()
            )));
        }
    }
    parent.ok_or_else(|| {
        Error::Tool(format!(
            "stale or unknown subagent child id `{}`",
            child.as_str()
        ))
    })
}
fn convert(error: imp_subagent::Error) -> Error {
    Error::Tool(error.to_string())
}
pub(super) fn status_name(status: &imp_subagent::Status) -> &'static str {
    match status {
        imp_subagent::Status::Pending => "pending",
        imp_subagent::Status::Running => "running",
        imp_subagent::Status::Success => "success",
        imp_subagent::Status::Blocked => "blocked",
        imp_subagent::Status::NeedsInput => "incomplete",
        imp_subagent::Status::Failed => "failed",
        imp_subagent::Status::Cancelled => "cancelled",
    }
}
fn map_status(status: &imp_subagent::Status) -> SubagentStatus {
    match status {
        imp_subagent::Status::Pending => SubagentStatus::Pending,
        imp_subagent::Status::Running => SubagentStatus::Running,
        imp_subagent::Status::Success => SubagentStatus::Success,
        imp_subagent::Status::Blocked => SubagentStatus::Blocked,
        imp_subagent::Status::NeedsInput => SubagentStatus::Incomplete,
        imp_subagent::Status::Failed => SubagentStatus::Failed,
        imp_subagent::Status::Cancelled => SubagentStatus::Cancelled,
    }
}
