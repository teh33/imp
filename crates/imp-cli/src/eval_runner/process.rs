use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;

#[derive(Debug, Clone, Copy)]
pub(super) struct ProcessOutcome {
    pub(super) exit_code: Option<i32>,
    pub(super) timed_out: bool,
    pub(super) duration_ms: u64,
}

impl ProcessOutcome {
    pub(super) fn success(self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

pub(super) async fn run_logged(
    program: &Path,
    args: &[String],
    cwd: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout: Duration,
) -> Result<ProcessOutcome, Box<dyn std::error::Error>> {
    run_logged_with_env(program, args, &[], cwd, stdout_path, stderr_path, timeout).await
}

pub(super) async fn run_logged_with_env(
    program: &Path,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout: Duration,
) -> Result<ProcessOutcome, Box<dyn std::error::Error>> {
    let (stdout, stderr) = create_outputs(stdout_path, stderr_path)?;
    let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let pwd = canonical_cwd.display().to_string();
    let mut child = Command::new(program)
        .args(args)
        .env("PWD", &pwd)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .current_dir(&canonical_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", program.display()))?;
    let started = Instant::now();

    let (exit_code, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => (status?.code(), false),
        Err(_) => {
            child.kill().await.ok();
            child.wait().await.ok();
            (None, true)
        }
    };

    Ok(ProcessOutcome {
        exit_code,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

pub(super) async fn run_capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("{program} {} failed: {stderr}", args.join(" ")).into())
    }
}

pub(super) async fn run_git(args: &[&str], cwd: &Path) -> Result<(), Box<dyn std::error::Error>> {
    run_capture("git", args, cwd).await.map(|_| ())
}

fn create_outputs(
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(File, File), Box<dyn std::error::Error>> {
    let stdout = create_output(stdout_path)?;
    let stderr = if stdout_path == stderr_path {
        stdout.try_clone()?
    } else {
        create_output(stderr_path)?
    };
    Ok((stdout, stderr))
}

fn create_output(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?)
}
