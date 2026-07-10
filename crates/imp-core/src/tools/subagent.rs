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
}

impl Default for SubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTool {
    pub fn new() -> Self {
        Self {
            executor: ImpSubagentExecutor::from_current_executable(),
        }
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
            "launch" => self.launch(
                params
                    .input
                    .ok_or_else(|| Error::Tool("missing `input` parameter".into()))?,
                &ctx,
            ),
            "status" => self.status(child_id(params.child_run_id)?, &ctx),
            "wait" => self.wait(
                child_id(params.child_run_id)?,
                params.timeout_seconds.unwrap_or(0),
                &ctx,
            ),
            "send" => self.send(
                child_id(params.child_run_id)?,
                params
                    .message
                    .ok_or_else(|| Error::Tool("missing `message` parameter".into()))?,
                &ctx,
            ),
            "cancel" => self.cancel(child_id(params.child_run_id)?, &ctx),
            action => Ok(ToolOutput::error(format!(
                "unsupported subagent action `{action}`"
            ))),
        }
    }
}

impl SubagentTool {
    fn launch(&self, input: SubagentInput, ctx: &ToolContext) -> Result<ToolOutput> {
        validate_launch_input(&input, ctx)?;
        let record = self.executor.launch(&input, ctx)?;
        Ok(output(
            "launch",
            &record,
            json!({"input": input, "record": record}),
            false,
        ))
    }
    fn status(&self, child: SubagentRunId, ctx: &ToolContext) -> Result<ToolOutput> {
        let record = self.executor.status(&ctx.cwd, &child)?;
        Ok(output(
            "status",
            &record,
            json!({"child_run_id": child, "record": record}),
            false,
        ))
    }
    fn wait(
        &self,
        child: SubagentRunId,
        timeout_seconds: u64,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let record = self.executor.wait(&ctx.cwd, &child, timeout_seconds)?;
        let outcome = self.executor.outcome(&record);
        Ok(output(
            "wait",
            &record,
            json!({"child_run_id": child, "record": record, "outcome": outcome}),
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
    fn cancel(&self, child: SubagentRunId, ctx: &ToolContext) -> Result<ToolOutput> {
        let record = self.executor.cancel(&ctx.cwd, &child)?;
        Ok(output(
            "cancel",
            &record,
            json!({"child_run_id": child, "record": record}),
            false,
        ))
    }
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
