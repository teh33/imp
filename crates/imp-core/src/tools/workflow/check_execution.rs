use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessManager, ProcessMode,
    ProcessRequest,
};
use crate::workflow::{CheckKind, WorkflowCheck};

const CHECK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const OUTPUT_RETENTION_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn evaluate(
    manager: &ProcessManager,
    check: &WorkflowCheck,
    cwd: &Path,
) -> Result<(String, String, Option<i32>), String> {
    match check.kind {
        CheckKind::Command => evaluate_command(manager, check, cwd, CHECK_TIMEOUT).await,
        CheckKind::ChangedFiles => evaluate_changed_files(manager, check, cwd).await,
        _ => Err(format!("check kind {:?} is not executable", check.kind)),
    }
}

async fn evaluate_command(
    manager: &ProcessManager,
    check: &WorkflowCheck,
    cwd: &Path,
    timeout: Duration,
) -> Result<(String, String, Option<i32>), String> {
    let command = check
        .command
        .as_deref()
        .ok_or_else(|| "command check is missing command".to_string())?;
    let output = run(manager, "sh", ["-c", command], cwd, timeout)
        .await
        .map_err(|error| format!("failed to run command check: {error}"))?;
    let zero_tests = output.success && has_zero_test_evidence(&output.stdout, &output.stderr);
    let status = if output.success && !zero_tests {
        "passed"
    } else {
        "failed"
    };
    let exit = output
        .exit_code
        .map_or_else(|| "signal".to_string(), |code| code.to_string());
    let reason = if output.timed_out {
        format!("command `{command}` timed out after {}s", timeout.as_secs())
    } else if zero_tests {
        format!(
            "command `{command}` exited with {exit} but matched zero tests; command checks require non-empty test evidence by default"
        )
    } else {
        format!("command `{command}` exited with {exit}")
    };
    Ok((status.into(), reason, output.exit_code))
}

async fn evaluate_changed_files(
    manager: &ProcessManager,
    check: &WorkflowCheck,
    cwd: &Path,
) -> Result<(String, String, Option<i32>), String> {
    if check.paths.is_empty() {
        return Err("changed_files check is missing paths".into());
    }
    let mut args = vec!["status".to_string(), "--porcelain".into(), "--".into()];
    args.extend(check.paths.iter().map(|path| path.display().to_string()));
    let output = run(manager, "git", args, cwd, CHECK_TIMEOUT)
        .await
        .map_err(|error| format!("failed to inspect git status: {error}"))?;
    if output.timed_out {
        return Err(format!(
            "git status timed out after {}s",
            CHECK_TIMEOUT.as_secs()
        ));
    }
    if !output.success {
        let exit = output
            .exit_code
            .map_or_else(|| "signal".to_string(), |code| code.to_string());
        return Err(format!("git status failed with {exit}"));
    }
    let status = if output.stdout.trim().is_empty() {
        "failed"
    } else {
        "passed"
    };
    Ok((
        status.into(),
        format!(
            "changed files check inspected {} path(s)",
            check.paths.len()
        ),
        output.exit_code,
    ))
}

struct Execution {
    success: bool,
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run<I, S>(
    manager: &ProcessManager,
    program: &str,
    args: I,
    cwd: &Path,
    timeout: Duration,
) -> Result<Execution, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let request = ProcessRequest {
        command: CommandSpec::new(program).with_arguments(args.into_iter().map(Into::into)),
        cwd: cwd.to_path_buf(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(timeout),
        output_retention_bytes: OUTPUT_RETENTION_BYTES,
        grant,
    };
    let process = manager
        .start(request)
        .await
        .map_err(|error| error.to_string())?;
    manager
        .close_stdin(process.id)
        .await
        .map_err(|error| error.to_string())?;
    let mut cursor = OutputCursor::start(process.id);
    let mut stdout = String::new();
    let mut stderr = String::new();
    loop {
        let output = manager
            .observe(process.id, cursor, Duration::from_millis(20), usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
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
    let exit = manager
        .wait(process.id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Execution {
        success: exit.code == Some(0) && !exit.timed_out && !exit.cancelled,
        timed_out: exit.timed_out,
        exit_code: exit.code,
        stdout,
        stderr,
    })
}

fn has_zero_test_evidence(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}{stderr}").to_lowercase();
    combined.contains("running 0 tests") || combined.contains("0 tests, 0 benchmarks")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_zero_test_output() {
        assert!(has_zero_test_evidence("running 0 tests", ""));
        assert!(!has_zero_test_evidence("running 1 test", ""));
    }

    #[tokio::test]
    async fn closes_stdin_and_bounds_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ProcessManager::new();
        let eof = run(
            &manager,
            "sh",
            ["-c", "if read line; then exit 1; else printf eof; fi"],
            dir.path(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(eof.stdout, "eof");
        let timeout = run(
            &manager,
            "sh",
            ["-c", "exec sleep 60"],
            dir.path(),
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert!(timeout.timed_out);
    }
}
