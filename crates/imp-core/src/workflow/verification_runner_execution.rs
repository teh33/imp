use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessError, ProcessExit,
    ProcessManager, ProcessMode, ProcessRequest,
};

const OUTPUT_RETENTION_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct VerificationExecution {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) stdout_bytes: usize,
    pub(super) stderr_bytes: usize,
    pub(super) exit: ProcessExit,
}

fn append_bounded(target: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(target.len());
    target.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

pub(super) async fn execute(
    manager: &ProcessManager,
    command: &str,
    cwd: &Path,
    timeout: Duration,
    max_capture_bytes: usize,
) -> Result<VerificationExecution, ProcessError> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let request = ProcessRequest {
        command: CommandSpec::new("/bin/sh").with_arguments(["-lc".into(), command.to_string()]),
        cwd: cwd.to_path_buf(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(timeout),
        output_retention_bytes: OUTPUT_RETENTION_BYTES,
        grant,
    };
    let process = manager.start(request).await?;
    manager.close_stdin(process.id).await?;
    collect(manager, process.id, max_capture_bytes).await
}

async fn collect(
    manager: &ProcessManager,
    id: crate::process::ProcessId,
    max_capture_bytes: usize,
) -> Result<VerificationExecution, ProcessError> {
    let mut cursor = OutputCursor::start(id);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_bytes: usize = 0;
    let mut stderr_bytes: usize = 0;
    loop {
        let output = manager
            .observe(id, cursor, Duration::from_millis(20), usize::MAX)
            .await?;
        cursor = output.next_cursor;
        for chunk in output.chunks {
            let byte_count = chunk.end.saturating_sub(chunk.start) as usize;
            match chunk.stream {
                OutputStream::Stdout => {
                    append_bounded(&mut stdout, chunk.text.as_bytes(), max_capture_bytes);
                    stdout_bytes = stdout_bytes.saturating_add(byte_count);
                }
                OutputStream::Stderr => {
                    append_bounded(&mut stderr, chunk.text.as_bytes(), max_capture_bytes);
                    stderr_bytes = stderr_bytes.saturating_add(byte_count);
                }
            }
        }
        if output.state.is_terminal() && !output.response_truncated {
            break;
        }
    }
    Ok(VerificationExecution {
        stdout,
        stderr,
        stdout_bytes,
        stderr_bytes,
        exit: manager.wait(id).await?,
    })
}
