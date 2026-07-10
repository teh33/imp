use std::path::{Path, PathBuf};

use serde_json::json;

use super::model::{map_loopr_status, status_name, LooprPoll, SubagentMapping};
use super::persistence::{load_mapping, load_parent_run, save_mapping, save_parent_run};
use super::SubagentTool;
use crate::agent::{SubagentEvent, SubagentInput, SubagentRunId};
use crate::error::{Error, Result};
use crate::tools::{ToolContext, ToolOutput};

impl SubagentTool {
    pub(super) async fn launch(
        &self,
        input: SubagentInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        validate_launch_input(&input, ctx)?;
        let existing_run_id = load_parent_run(&ctx.cwd, input.parent_run_id.as_str())?;
        let mapping = self
            .executor
            .launch(&input, &ctx.cwd, existing_run_id)
            .await?;
        save_parent_run(
            &ctx.cwd,
            mapping.parent_run_id.as_str(),
            &mapping.loopr_run_id,
        )?;
        save_mapping(&ctx.cwd, &mapping)?;
        let event = SubagentEvent::Started {
            child_run_id: input.child_run_id.clone(),
            role: input.role.clone(),
            objective: input.objective.clone(),
        };
        Ok(output(
            "launch",
            mapping.status.clone(),
            json!({
                "child_run_id": input.child_run_id,
                "event": event,
                "unsupported_resource_limits": mapping.unsupported_resource_limits.clone(),
                "mapping": mapping,
            }),
            false,
        ))
    }

    pub(super) async fn status(
        &self,
        child: SubagentRunId,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let mut mapping = load_mapping(&ctx.cwd, &child)?;
        let poll = self.executor.poll(&mapping, &ctx.cwd).await?;
        let status = update_status(&mut mapping, &poll);
        save_mapping(&ctx.cwd, &mapping)?;
        Ok(output(
            "status",
            status,
            json!({ "child_run_id": child, "poll": poll, "mapping": mapping }),
            false,
        ))
    }

    pub(super) async fn wait(
        &self,
        child: SubagentRunId,
        timeout: u64,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let mut mapping = load_mapping(&ctx.cwd, &child)?;
        let timeout = mapping
            .timeout_seconds
            .map(|limit| timeout.min(limit))
            .unwrap_or(timeout);
        let waited = self.executor.wait(&mapping, &ctx.cwd, timeout).await;
        let (poll, wait_error) = match waited {
            Ok(poll) => (poll, None),
            Err(error) => (
                self.executor.poll(&mapping, &ctx.cwd).await?,
                Some(error.to_string()),
            ),
        };
        let status = update_status(&mut mapping, &poll);
        mapping.result = poll.result.clone();
        save_mapping(&ctx.cwd, &mapping)?;
        if wait_error.is_some() && !status.is_terminal() {
            return Err(Error::Tool(wait_error.unwrap()));
        }
        let mut diagnostics = wait_error.into_iter().collect::<Vec<_>>();
        if let Some(error) = poll.child_status_error.as_deref() {
            diagnostics.push(format!("loopr child status diagnostic: {error}"));
        }
        let outcome = mapping.outcome(
            status.clone(),
            poll.result.clone(),
            poll.child_status.clone(),
            diagnostics,
        );
        Ok(output(
            "wait",
            status,
            json!({ "child_run_id": child, "outcome": outcome, "poll": poll }),
            false,
        ))
    }

    pub(super) async fn send(
        &self,
        child: SubagentRunId,
        message: String,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        if message.trim().is_empty() {
            return Err(Error::Tool(
                "subagent send requires a non-empty message".into(),
            ));
        }
        let mut mapping = load_mapping(&ctx.cwd, &child)?;
        let sent = self.executor.send(&mapping, &ctx.cwd, &message).await?;
        if !sent.sent || sent.thread != mapping.loopr_thread_id {
            return Err(Error::Tool(
                "loopr did not acknowledge the expected child follow-up".into(),
            ));
        }
        mapping.loopr_session_id = sent.session.clone().or(mapping.loopr_session_id);
        mapping.loopr_turn_id = sent.turn.clone().or(mapping.loopr_turn_id);
        mapping.status = crate::agent::SubagentStatus::Running;
        save_mapping(&ctx.cwd, &mapping)?;
        Ok(output(
            "send",
            mapping.status.clone(),
            json!({ "child_run_id": child, "sent": sent, "mapping": mapping }),
            false,
        ))
    }

    pub(super) async fn cancel(
        &self,
        child: SubagentRunId,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let mut mapping = load_mapping(&ctx.cwd, &child)?;
        let poll = self.executor.stop(&mapping, &ctx.cwd).await?;
        let status = update_status(&mut mapping, &poll);
        if status != crate::agent::SubagentStatus::Cancelled {
            return Err(Error::Tool(format!(
                "loopr stop was not observed as cancelled; observed {}",
                status_name(&status)
            )));
        }
        save_mapping(&ctx.cwd, &mapping)?;
        Ok(output(
            "cancel",
            status,
            json!({ "child_run_id": child, "poll": poll, "mapping": mapping }),
            false,
        ))
    }
}

pub(super) fn child_id(id: Option<SubagentRunId>) -> Result<SubagentRunId> {
    let id = id.ok_or_else(|| Error::Tool("missing `child_run_id` parameter".into()))?;
    valid_id(id.as_str())?;
    Ok(id)
}

fn validate_launch_input(input: &SubagentInput, ctx: &ToolContext) -> Result<()> {
    valid_id(input.child_run_id.as_str())?;
    if input.parent_run_id.as_str().trim().is_empty() {
        return Err(Error::Tool(
            "subagent launch requires a non-empty parent_run_id".into(),
        ));
    }
    valid_id(input.parent_run_id.as_str())?;
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

fn ensure_contract_path(path: &Path, ctx: &ToolContext) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    };
    let root = canonicalize_missing(&ctx.cwd);
    let candidate = canonicalize_missing(&absolute);
    if !candidate.starts_with(root) {
        return Err(Error::Tool(format!(
            "subagent launch denied: context path {} is outside parent cwd",
            path.display()
        )));
    }
    Ok(())
}

fn canonicalize_missing(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut suffix = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(mut canonical) = parent.canonicalize() {
            for name in suffix.iter().rev() {
                canonical.push(name);
            }
            return canonical;
        }
        current = parent;
    }
    path.to_path_buf()
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::Tool(
            "subagent child and parent ids must contain only ASCII letters, numbers, '-' or '_'"
                .into(),
        ));
    }
    Ok(())
}

fn update_status(mapping: &mut SubagentMapping, poll: &LooprPoll) -> crate::agent::SubagentStatus {
    let status = map_loopr_status(&poll.status, poll.child_status.as_ref());
    mapping.status = status.clone();
    status
}

fn output(
    action: &str,
    status: crate::agent::SubagentStatus,
    details: serde_json::Value,
    is_error: bool,
) -> ToolOutput {
    let child = details["child_run_id"]
        .as_str()
        .or_else(|| details["mapping"]["child_run_id"].as_str())
        .unwrap_or("");
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: format!("Subagent {action}: {child} [{}]", status_name(&status)),
        }],
        details: json!({ "action": action, "status": status_name(&status), "result": details }),
        is_error,
    }
}
