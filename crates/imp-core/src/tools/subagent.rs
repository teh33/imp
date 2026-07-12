mod contract;
mod executor;

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use self::executor::ImpSubagentExecutor;
use super::{Tool, ToolContext, ToolOutput};
use crate::agent::{SubagentInput, SubagentRunId};
use crate::error::{Error, Result};

pub struct SubagentTool {
    executor: ImpSubagentExecutor,
    default_model: Option<String>,
}

impl Default for SubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTool {
    pub fn new() -> Self {
        Self::with_default_model(None)
    }

    pub fn with_default_model(default_model: Option<String>) -> Self {
        Self {
            executor: ImpSubagentExecutor::from_current_executable(),
            default_model,
        }
    }

    fn resolve_model(&self, mut input: SubagentInput) -> Result<SubagentInput> {
        input.model = Some(self.resolved_model(input.model)?);
        Ok(input)
    }

    fn resolved_model(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| Error::Tool("subagent launch requires a resolved model".into()))
    }
}

#[derive(Debug, Deserialize)]
struct SubagentParams {
    action: String,
    input: Option<SubagentInput>,
    child_run_id: Option<SubagentRunId>,
    message: Option<String>,
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }
    fn label(&self) -> &str {
        "Subagent"
    }
    fn description(&self) -> &str {
        "Launch and manage policy-bounded imp subagents from workflow-generated contracts."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","required":["action"],"properties":{
            "action":{"type":"string","enum":["launch","status","wait","send","cancel"]},
            "input":{"type":"object","description":"Workflow-generated SubagentInput required for launch."},
            "child_run_id":{"type":"string","description":"Previously launched imp child id."},
            "message":{"type":"string","description":"Follow-up required for send."},
            "timeout_seconds":{"type":"integer","minimum":0}
        }})
    }
    fn is_readonly(&self) -> bool {
        false
    }
    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let params: SubagentParams = serde_json::from_value(params)
            .map_err(|error| Error::Tool(format!("invalid subagent params: {error}")))?;
        match params.action.as_str() {
            "launch" => {
                self.launch(
                    self.resolve_model(
                        params
                            .input
                            .ok_or_else(|| Error::Tool("missing `input` parameter".into()))?,
                    )?,
                    &ctx,
                )
                .await
            }
            "status" => self.status(child_id(params.child_run_id)?, &ctx).await,
            "wait" => {
                self.wait(
                    child_id(params.child_run_id)?,
                    params.timeout_seconds.unwrap_or(0),
                    &ctx,
                )
                .await
            }
            "send" => self.send(
                child_id(params.child_run_id)?,
                params
                    .message
                    .ok_or_else(|| Error::Tool("missing `message` parameter".into()))?,
                &ctx,
            ),
            "cancel" => self.cancel(child_id(params.child_run_id)?, &ctx).await,
            action => Ok(ToolOutput::error(format!(
                "unsupported subagent action `{action}`"
            ))),
        }
    }
}

impl SubagentTool {
    async fn launch(&self, input: SubagentInput, ctx: &ToolContext) -> Result<ToolOutput> {
        validate_launch_input(&input, ctx)?;
        let workspace = match workspace_request(&input) {
            None => None,
            Some(request) => Some(
                crate::managed_workspace::ManagedWorkspaceService::global()
                    .create(&ctx.cwd, request)
                    .await
                    .map_err(|error| {
                        Error::Tool(format!("subagent workspace creation failed: {error}"))
                    })?,
            ),
        };
        let child_cwd = workspace
            .as_ref()
            .map(|record| record.worktree_path.as_path())
            .unwrap_or(&ctx.cwd);
        let record = match self
            .executor
            .launch(&input, ctx, child_cwd, workspace.as_ref())
        {
            Ok(record) => record,
            Err(error) => {
                if let Some(workspace) = &workspace {
                    crate::managed_workspace::ManagedWorkspaceService::global()
                        .retain(&ctx.cwd, workspace.id.as_str(), error.to_string())
                        .await
                        .map_err(|retain_error| {
                            Error::Tool(format!(
                                "{error}; failed to retain subagent workspace: {retain_error}"
                            ))
                        })?;
                }
                return Err(error);
            }
        };
        Ok(output(
            "launch",
            &record,
            json!({"input": input, "record": record, "workspace": workspace}),
            false,
        ))
    }
    async fn status(&self, child: SubagentRunId, ctx: &ToolContext) -> Result<ToolOutput> {
        let record = self.executor.status(&ctx.cwd, &child)?;
        let workspace = reconcile_terminal_workspace(&record, ctx).await?;
        Ok(output(
            "status",
            &record,
            json!({"child_run_id": child, "record": record, "workspace": workspace}),
            false,
        ))
    }
    async fn wait(
        &self,
        child: SubagentRunId,
        timeout_seconds: u64,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let record = self.executor.wait(&ctx.cwd, &child, timeout_seconds)?;
        let outcome = self.executor.outcome(&record);
        let workspace = reconcile_terminal_workspace(&record, ctx).await?;
        Ok(output(
            "wait",
            &record,
            json!({"child_run_id": child, "record": record, "outcome": outcome, "workspace": workspace}),
            false,
        ))
    }
    fn send(&self, child: SubagentRunId, message: String, ctx: &ToolContext) -> Result<ToolOutput> {
        if message.trim().is_empty() {
            return Err(Error::Tool(
                "subagent send requires a non-empty message".into(),
            ));
        }
        let record = self.executor.send(&ctx.cwd, &child, message)?;
        Ok(output(
            "send",
            &record,
            json!({"child_run_id": child, "record": record}),
            false,
        ))
    }
    async fn cancel(&self, child: SubagentRunId, ctx: &ToolContext) -> Result<ToolOutput> {
        let record = self.executor.cancel(&ctx.cwd, &child)?;
        let workspace = reconcile_terminal_workspace(&record, ctx).await?;
        Ok(output(
            "cancel",
            &record,
            json!({"child_run_id": child, "record": record, "workspace": workspace}),
            false,
        ))
    }
}

fn managed_workspace_id(record: &imp_subagent::Record) -> Option<&str> {
    record.metadata["managed_workspace_id"].as_str()
}

async fn reconcile_terminal_workspace(
    record: &imp_subagent::Record,
    ctx: &ToolContext,
) -> Result<Option<crate::managed_workspace::ManagedWorkspaceRecord>> {
    let Some(id) = managed_workspace_id(record) else {
        return Ok(None);
    };
    if !record.status.terminal() {
        return Ok(None);
    }
    let service = crate::managed_workspace::ManagedWorkspaceService::global();
    let workspace = if record.status == imp_subagent::Status::Success {
        service.inspect(&ctx.cwd, id).await
    } else {
        service
            .retain(&ctx.cwd, id, terminal_workspace_diagnostic(record))
            .await
    };
    workspace.map(Some).map_err(|error| {
        Error::Tool(format!(
            "failed to reconcile subagent workspace `{id}`: {error}"
        ))
    })
}

fn terminal_workspace_diagnostic(record: &imp_subagent::Record) -> String {
    let status = executor::status_name(&record.status);
    match record.diagnostics.as_slice() {
        [] => format!("subagent finished with status {status}"),
        diagnostics => format!(
            "subagent finished with status {status}: {}",
            diagnostics.join("; ")
        ),
    }
}

fn workspace_request(
    input: &SubagentInput,
) -> Option<crate::managed_workspace::CreateManagedWorkspace> {
    (!input.resource_limits.writable_paths.is_empty()).then(|| {
        crate::managed_workspace::CreateManagedWorkspace {
            id: Some(format!(
                "subagent-{}",
                input.child_run_id.as_str().replace('_', "-")
            )),
            run_id: input.child_run_id.as_str().to_string(),
            task: Some(input.objective.clone()),
            base_ref: None,
        }
    })
}

fn validate_launch_input(input: &SubagentInput, ctx: &ToolContext) -> Result<()> {
    valid_id(input.parent_run_id.as_str())?;
    valid_id(input.child_run_id.as_str())?;
    if input.objective.trim().is_empty() {
        return Err(Error::Tool(
            "subagent launch requires a non-empty objective".into(),
        ));
    }
    for path in input
        .resource_limits
        .allowed_paths
        .iter()
        .chain(&input.resource_limits.writable_paths)
    {
        ensure_contract_path(path, ctx)?;
    }
    for path in &input.resource_limits.writable_paths {
        ctx.check_write_path(path)
            .map_err(|reason| Error::Tool(format!("subagent launch denied: {reason}")))?;
    }
    Ok(())
}
fn child_id(value: Option<SubagentRunId>) -> Result<SubagentRunId> {
    let value = value.ok_or_else(|| Error::Tool("missing `child_run_id` parameter".into()))?;
    valid_id(value.as_str())?;
    Ok(value)
}
fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(Error::Tool(
            "subagent ids must contain only ASCII letters, numbers, '-' or '_'".into(),
        ))
    } else {
        Ok(())
    }
}
fn ensure_contract_path(path: &std::path::Path, ctx: &ToolContext) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    };
    let candidate = canonicalize_missing(&absolute);
    if !candidate.starts_with(canonicalize_missing(&ctx.cwd)) {
        return Err(Error::Tool(format!(
            "subagent launch denied: context path {} is outside parent cwd",
            path.display()
        )));
    }
    Ok(())
}
fn canonicalize_missing(path: &std::path::Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let mut suffix = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            suffix.push(name);
        }
        if let Ok(mut resolved) = parent.canonicalize() {
            for name in suffix.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        current = parent;
    }
    path.to_path_buf()
}
fn output(
    action: &str,
    record: &imp_subagent::Record,
    details: serde_json::Value,
    is_error: bool,
) -> ToolOutput {
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: format!(
                "Subagent {action}: {} [{}]",
                record.child_id,
                executor::status_name(&record.status)
            ),
        }],
        details: json!({"action":action,"status":executor::status_name(&record.status),"result":details}),
        is_error,
    }
}

#[cfg(test)]
mod tests;
