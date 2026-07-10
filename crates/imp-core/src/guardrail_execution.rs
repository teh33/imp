use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessError, ProcessManager,
    ProcessMode, ProcessRequest,
};

const CAPTURE_BYTES: usize = 16 * 1024;
const RETENTION_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct GuardrailExecution {
    pub(super) success: bool,
    pub(super) stdout: String,
    pub(super) stderr: String,
    timed_out: bool,
}

pub(super) async fn execute(
    manager: &ProcessManager,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> io::Result<GuardrailExecution> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let request = ProcessRequest {
        command: CommandSpec::new("sh").with_arguments(["-c".into(), command.to_string()]),
        cwd: cwd.to_path_buf(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(timeout),
        output_retention_bytes: RETENTION_BYTES,
        grant,
    };
    let process = manager.start(request).await.map_err(process_error)?;
    manager
        .close_stdin(process.id)
        .await
        .map_err(process_error)?;
    let execution = collect(manager, process.id).await.map_err(process_error)?;
    if execution.timed_out {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("guardrail command timed out after {}s", timeout.as_secs()),
        ));
    }
    Ok(execution)
}

async fn collect(
    manager: &ProcessManager,
    id: crate::process::ProcessId,
) -> Result<GuardrailExecution, ProcessError> {
    let mut cursor = OutputCursor::start(id);
    let mut stdout = String::new();
    let mut stderr = String::new();
    loop {
        let output = manager
            .observe(id, cursor, Duration::from_millis(20), usize::MAX)
            .await?;
        cursor = output.next_cursor;
        for chunk in output.chunks {
            match chunk.stream {
                OutputStream::Stdout => append_bounded(&mut stdout, &chunk.text),
                OutputStream::Stderr => append_bounded(&mut stderr, &chunk.text),
            }
        }
        if output.state.is_terminal() && !output.response_truncated {
            break;
        }
    }
    let exit = manager.wait(id).await?;
    Ok(GuardrailExecution {
        success: exit.code == Some(0) && !exit.timed_out && !exit.cancelled,
        stdout,
        stderr,
        timed_out: exit.timed_out,
    })
}

fn append_bounded(target: &mut String, value: &str) {
    let remaining = CAPTURE_BYTES.saturating_sub(target.len());
    if remaining == 0 {
        return;
    }
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if next > remaining {
            break;
        }
        end = next;
    }
    target.push_str(&value[..end]);
}

fn process_error(error: ProcessError) -> io::Error {
    match error {
        ProcessError::Spawn(error) | ProcessError::Io(error) => error,
        error => io::Error::other(error.to_string()),
    }
}
