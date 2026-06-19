use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) async fn run_git<I, S>(cwd: &Path, args: I) -> std::io::Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    run_git_command(command).await
}

pub(super) async fn run_git_owned(
    cwd: &Path,
    args: Vec<String>,
) -> std::io::Result<std::process::Output> {
    run_git(cwd, args).await
}

pub(super) async fn run_git_with_env<I, S>(
    cwd: &Path,
    args: I,
    temp_index: Option<(&str, &Path)>,
) -> std::io::Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some((index, work_tree)) = temp_index {
        command
            .env("GIT_INDEX_FILE", index)
            .env("GIT_WORK_TREE", work_tree);
    }
    run_git_command(command).await
}

async fn run_git_command(mut command: Command) -> std::io::Result<std::process::Output> {
    match tokio::time::timeout(GIT_COMMAND_TIMEOUT, command.output()).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "git command timed out after {}s",
                GIT_COMMAND_TIMEOUT.as_secs()
            ),
        )),
    }
}

pub(super) async fn run_git_owned_with_env(
    cwd: &Path,
    args: Vec<String>,
    temp_index: Option<(&str, &Path)>,
) -> std::io::Result<std::process::Output> {
    run_git_with_env(cwd, args, temp_index).await
}
