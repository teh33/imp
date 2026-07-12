use std::collections::BTreeSet;
use std::process::Stdio;

use tokio::process::Command;

use super::{
    EnforcementSummary, OutputCursor, ProcessId, ProcessInfo, ProcessRequest, ProcessState,
};

pub(super) fn build_command(request: &ProcessRequest) -> Command {
    let mut command = Command::new(&request.command.program);
    command
        .args(&request.command.arguments)
        .current_dir(&request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for (name, value) in &request.environment {
        command.env(name, value);
    }
    for secret in &request.approved_secret_environment {
        command.env(&secret.name, &secret.value);
    }
    command
}

pub(super) fn process_info(
    id: ProcessId,
    request: &ProcessRequest,
    enforcement: EnforcementSummary,
    started_at_ms: u64,
) -> ProcessInfo {
    ProcessInfo {
        id,
        program: request.command.program.clone(),
        cwd: request.cwd.clone(),
        state: ProcessState::Starting,
        started_at_ms,
        exit: None,
        next_cursor: OutputCursor::start(id),
        enforcement,
        approved_secret_ids: request
            .approved_secret_environment
            .iter()
            .map(|secret| secret.secret_id.clone())
            .collect::<BTreeSet<_>>(),
        approved_secret_environment: request
            .approved_secret_environment
            .iter()
            .map(|secret| secret.name.clone())
            .collect::<BTreeSet<_>>(),
    }
}
