//! Process-group primitives used by the owned process runtime.

use std::process::{Child, Command};

use tokio::process::Command as TokioCommand;

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
    command.process_group(0);
}

/// Send SIGKILL to a blocking child's process group.
#[allow(dead_code)]
pub(crate) fn kill_std_process_group(child: &Child) {
    signal_process_group(child.id(), ProcessGroupSignal::Kill);
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProcessGroupSignal {
    Interrupt,
    Terminate,
    Kill,
    Hangup,
}

/// Signal the process group rooted at `pid`.
pub(crate) fn signal_process_group(pid: u32, signal: ProcessGroupSignal) {
    #[cfg(unix)]
    if let Ok(raw_pid) = i32::try_from(pid) {
        if let Some(pid) = rustix::process::Pid::from_raw(raw_pid) {
            let signal = match signal {
                ProcessGroupSignal::Interrupt => rustix::process::Signal::INT,
                ProcessGroupSignal::Terminate => rustix::process::Signal::TERM,
                ProcessGroupSignal::Kill => rustix::process::Signal::KILL,
                ProcessGroupSignal::Hangup => rustix::process::Signal::HUP,
            };
            let _ = rustix::process::kill_process_group(pid, signal);
        }
    }

    #[cfg(not(unix))]
    let _ = (pid, signal);
}

/// Send SIGKILL to the process group created for an async child process.
pub(crate) async fn kill_tokio_process_group(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        signal_process_group(pid, ProcessGroupSignal::Kill);
    }
}
