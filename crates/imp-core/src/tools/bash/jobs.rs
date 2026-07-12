use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};

use super::{build_process_request, resolve_with_secrets, sanitize_output_text, RequestedSecret};
use crate::error::{Error, Result};
use crate::process::{OutputCursor, ProcessId, ProcessManager, ProcessOutput, StopOptions};
use crate::tools::{ToolContext, ToolOutput};

const DEFAULT_JOB_WAIT_MS: u64 = 500;
const MAX_JOB_WAIT_MS: u64 = 5_000;
const JOB_READ_BYTES: usize = 50 * 1024;

#[derive(Default)]
pub(super) struct BashJobs {
    cursors: Mutex<HashMap<ProcessId, OutputCursor>>,
}

impl BashJobs {
    pub(super) async fn start(
        &self,
        manager: &ProcessManager,
        command: &str,
        timeout_secs: u64,
        ctx: &ToolContext,
        with_secrets: Vec<RequestedSecret>,
        params: &Value,
    ) -> Result<ToolOutput> {
        if ctx.is_cancelled() {
            return Ok(ToolOutput::error("Tool execution cancelled."));
        }
        let resolved = resolve_with_secrets(ctx, with_secrets)?;
        let request = build_process_request(command, timeout_secs, ctx, &resolved);
        let info = manager
            .start(request)
            .await
            .map_err(|error| Error::Tool(error.to_string()))?;
        self.prune(manager);
        self.remember(info.id);
        let output = manager
            .observe(
                info.id,
                OutputCursor::start(info.id),
                Duration::from_millis(job_wait_ms(params)),
                JOB_READ_BYTES,
            )
            .await
            .map_err(|error| Error::Tool(error.to_string()))?;
        lock(&self.cursors).insert(info.id, output.next_cursor);
        Ok(job_output(manager, output, false))
    }

    pub(super) fn remember(&self, id: ProcessId) {
        lock(&self.cursors).insert(id, OutputCursor::start(id));
    }

    pub(super) async fn interact(
        &self,
        manager: &ProcessManager,
        params: &Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let id = parse_job_id(params)?;
        if ctx.is_cancelled() {
            let _ = manager.cancel(id).await;
        }
        if let Some(stdin) = params.get("stdin").and_then(Value::as_str) {
            manager
                .write(id, stdin.as_bytes())
                .await
                .map_err(|error| Error::Tool(error.to_string()))?;
        }
        let stopped = params.get("stop").and_then(Value::as_bool).unwrap_or(false);
        if stopped {
            manager
                .stop(id, StopOptions::default())
                .await
                .map_err(|error| Error::Tool(error.to_string()))?;
        }
        let cursor = lock(&self.cursors)
            .get(&id)
            .copied()
            .ok_or_else(|| Error::Tool(format!("unknown bash job {id}")))?;
        let wait = if stopped {
            Duration::ZERO
        } else {
            Duration::from_millis(job_wait_ms(params))
        };
        let output = manager
            .observe(id, cursor, wait, JOB_READ_BYTES)
            .await
            .map_err(|error| Error::Tool(error.to_string()))?;
        lock(&self.cursors).insert(id, output.next_cursor);
        Ok(job_output(manager, output, stopped))
    }

    fn prune(&self, manager: &ProcessManager) {
        lock(&self.cursors).retain(|id, _| manager.get(*id).is_ok());
    }
}

pub(super) fn job_wait_ms(params: &Value) -> u64 {
    params
        .get("yield_time_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_JOB_WAIT_MS)
        .min(MAX_JOB_WAIT_MS)
}

pub(super) fn job_output(
    manager: &ProcessManager,
    output: ProcessOutput,
    stopped: bool,
) -> ToolOutput {
    let info = manager.get(output.process_id).ok();
    let mut text = output
        .chunks
        .iter()
        .map(|chunk| sanitize_output_text(&chunk.text))
        .collect::<String>();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let state = output.state;
    if state.is_terminal() {
        let code = info
            .as_ref()
            .and_then(|item| item.exit.as_ref())
            .and_then(|exit| exit.code);
        text.push_str(&code.map_or_else(
            || format!("[Job {} ended: {state:?}]", output.process_id),
            |code| format!("[Job {} exited with code {code}]", output.process_id),
        ));
    } else if stopped {
        text.push_str(&format!("[Job {} stopped]", output.process_id));
    } else {
        text.push_str(&format!("[Job {} is running]", output.process_id));
    }
    let exit = info.as_ref().and_then(|item| item.exit.clone());
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "job_id": output.process_id,
            "state": state,
            "exit": exit,
            "unread_output_evicted": output.unread_output_evicted,
            "truncated": output.response_truncated,
        }),
        is_error: !stopped
            && exit.as_ref().is_some_and(|exit| {
                exit.timed_out || exit.cancelled || exit.code.is_some_and(|code| code != 0)
            }),
    }
}

fn parse_job_id(params: &Value) -> Result<ProcessId> {
    let raw = params
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Tool("missing 'job_id' parameter".into()))?;
    ProcessId::parse(raw).map_err(|_| Error::Tool("invalid job_id".into()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
