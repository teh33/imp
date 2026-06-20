//! Helpers for launching and cleaning up child process groups.
//!
//! Tools run user-provided or extension-provided commands that may spawn
//! grandchildren. Put each command in its own process group so timeout and
//! cancellation cleanup can target the whole tree instead of just the shell.

use std::process::{Child, Command};

use tokio::process::{Child as TokioChild, Command as TokioCommand};

/// Put a blocking child command in a new process group on Unix.
#[allow(dead_code)]
pub(crate) fn isolate_std_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
}

/// Put an async child command in a new process group on Unix.
pub(crate) fn isolate_tokio_command(command: &mut TokioCommand) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

/// Send SIGKILL to the process group created for a blocking child process.
#[allow(dead_code)]
pub(crate) fn kill_std_process_group(child: &Child) {
    #[cfg(unix)]
    if let Some(pid) = child_pid(child.id()) {
        send_unix_signal_to_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Send SIGKILL to the process group created for an async child process.
pub(crate) async fn kill_tokio_process_group(child: &TokioChild) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(child_pid) {
        send_unix_signal_to_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Send SIGTERM to an async child process group, then SIGKILL after a grace period.
pub(crate) async fn terminate_tokio_process_group(child: &TokioChild) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(child_pid) {
        send_unix_signal_to_process_group(pid, rustix::process::Signal::TERM);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        send_unix_signal_to_process_group(pid, rustix::process::Signal::KILL);
    }
}

#[cfg(unix)]
fn child_pid(pid: u32) -> Option<i32> {
    i32::try_from(pid).ok()
}

#[cfg(unix)]
fn send_unix_signal_to_process_group(pid: i32, signal: rustix::process::Signal) {
    if let Some(pid) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}
