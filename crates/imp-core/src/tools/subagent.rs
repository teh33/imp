mod adapter;
mod lifecycle;
mod model;
mod persistence;

#[cfg(test)]
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use self::adapter::LooprSubagentExecutor;
use self::lifecycle::child_id;
use super::{Tool, ToolContext, ToolOutput};
use crate::agent::{SubagentInput, SubagentRunId};
use crate::error::{Error, Result};

pub struct SubagentTool {
    executor: LooprSubagentExecutor,
}

impl Default for SubagentTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTool {
    pub fn new() -> Self {
        Self {
            executor: LooprSubagentExecutor::default(),
        }
    }

    #[cfg(test)]
    fn with_executable_and_timeout(executable: PathBuf, timeout: std::time::Duration) -> Self {
        Self {
            executor: LooprSubagentExecutor::with_timeout(executable, timeout),
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
        "Launch and manage policy-bounded loopr subagents from workflow-generated contracts."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object", "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["launch", "status", "wait", "send", "cancel"] },
                "input": { "type": "object", "description": "Workflow-generated SubagentInput, required only for launch." },
                "child_run_id": { "type": "string", "description": "Previously launched imp child id, required except for launch." },
                "message": { "type": "string", "description": "Follow-up message, required for send." },
                "timeout_seconds": { "type": "integer", "minimum": 0, "description": "Wait limit in seconds." }
            }
        })
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
                    params
                        .input
                        .ok_or_else(|| Error::Tool("missing `input` parameter".into()))?,
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
            "send" => {
                self.send(
                    child_id(params.child_run_id)?,
                    params
                        .message
                        .ok_or_else(|| Error::Tool("missing `message` parameter".into()))?,
                    &ctx,
                )
                .await
            }
            "cancel" => self.cancel(child_id(params.child_run_id)?, &ctx).await,
            other => Ok(ToolOutput::error(format!(
                "unsupported subagent action `{other}`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
