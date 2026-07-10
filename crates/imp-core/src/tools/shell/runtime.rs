use std::collections::BTreeMap;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessError, ProcessExit,
    ProcessManager, ProcessMode, ProcessRequest,
};
use crate::tools::ToolContext;

use super::ShellToolDef;

const OUTPUT_RETENTION_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct ShellExecution {
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) exit: ProcessExit,
}

pub(super) async fn execute(
    manager: &ProcessManager,
    def: &ShellToolDef,
    args: Vec<String>,
    ctx: &ToolContext,
) -> Result<ShellExecution, ProcessError> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let mut grant = ExecutionGrant::host(&ctx.cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let request = ProcessRequest {
        command: CommandSpec::new(&def.exec.command).with_arguments(args),
        cwd: ctx.cwd.clone(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(Duration::from_secs(def.exec.timeout as u64)),
        output_retention_bytes: OUTPUT_RETENTION_BYTES,
        grant,
    };
    let process = manager.start(request).await?;
    manager.close_stdin(process.id).await?;
    collect(manager, process.id, ctx).await
}

async fn collect(
    manager: &ProcessManager,
    id: crate::process::ProcessId,
    ctx: &ToolContext,
) -> Result<ShellExecution, ProcessError> {
    let mut cursor = OutputCursor::start(id);
    let mut stdout = String::new();
    let mut stderr = String::new();
    loop {
        if ctx.is_cancelled() {
            manager.cancel(id).await?;
        }
        let output = manager
            .observe(id, cursor, Duration::from_millis(20), usize::MAX)
            .await?;
        cursor = output.next_cursor;
        for chunk in output.chunks {
            match chunk.stream {
                OutputStream::Stdout => stdout.push_str(&chunk.text),
                OutputStream::Stderr => stderr.push_str(&chunk.text),
            }
        }
        if output.state.is_terminal() && !output.response_truncated {
            break;
        }
    }
    Ok(ShellExecution {
        stdout,
        stderr,
        exit: manager.wait(id).await?,
    })
}
