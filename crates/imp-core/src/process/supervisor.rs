use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc};

use crate::child_process::{signal_process_group, ProcessGroupSignal};

use super::manager::{lock, Control, ProcessRecord};
use super::{ProcessEvent, ProcessExit, ProcessSignal, ProcessState};

pub(super) fn spawn_supervisor(
    mut child: tokio::process::Child,
    os_pid: Option<u32>,
    timeout: Option<Duration>,
    mut control: mpsc::UnboundedReceiver<Control>,
    drain_tasks: [tokio::task::JoinHandle<()>; 2],
    record: Arc<ProcessRecord>,
    events: broadcast::Sender<ProcessEvent>,
    started_at_ms: u64,
) {
    tokio::spawn(async move {
        let deadline = timeout.map(|duration| tokio::time::Instant::now() + duration);
        let classification = loop {
            tokio::select! {
                status = child.wait() => break (status.ok(), false, false),
                message = control.recv() => match message {
                    Some(Control::Signal(signal)) => signal_os(os_pid, signal),
                    Some(Control::Stop(options)) => {
                        terminate(os_pid, options.grace, &mut child).await;
                        break (child.wait().await.ok(), false, true);
                    }
                    Some(Control::Cancel) | None => {
                        kill(os_pid, &mut child).await;
                        break (child.wait().await.ok(), false, true);
                    }
                },
                _ = wait_deadline(deadline), if deadline.is_some() => {
                    kill(os_pid, &mut child).await;
                    break (child.wait().await.ok(), true, false);
                }
            }
        };
        *record.stdin.lock().await = None;
        for task in drain_tasks {
            let _ = task.await;
        }
        finish(&record, &events, classification, started_at_ms);
    });
}

async fn wait_deadline(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    }
}

async fn terminate(pid: Option<u32>, grace: Duration, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        signal_process_group(pid, ProcessGroupSignal::Terminate);
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        kill(pid, child).await;
    }
}

async fn kill(pid: Option<u32>, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        signal_process_group(pid, ProcessGroupSignal::Kill);
    }
    let _ = child.start_kill();
}

fn signal_os(pid: Option<u32>, signal: ProcessSignal) {
    let Some(pid) = pid else { return };
    let signal = match signal {
        ProcessSignal::Interrupt => ProcessGroupSignal::Interrupt,
        ProcessSignal::Terminate => ProcessGroupSignal::Terminate,
        ProcessSignal::Kill => ProcessGroupSignal::Kill,
        ProcessSignal::Hangup => ProcessGroupSignal::Hangup,
    };
    signal_process_group(pid, signal);
}

fn finish(
    record: &ProcessRecord,
    events: &broadcast::Sender<ProcessEvent>,
    classification: (Option<std::process::ExitStatus>, bool, bool),
    started_at_ms: u64,
) {
    let (status, timed_out, cancelled) = classification;
    let exit = ProcessExit {
        code: status.and_then(|status| status.code()),
        signal: exit_signal(status),
        timed_out,
        cancelled,
        started_at_ms,
        exited_at_ms: now_ms(),
    };
    let state = if timed_out {
        ProcessState::TimedOut
    } else if cancelled {
        ProcessState::Cancelled
    } else if status.is_some() {
        ProcessState::Exited
    } else {
        ProcessState::Failed
    };
    let id = {
        let mut info = lock(&record.info);
        info.state = state;
        info.exit = Some(exit.clone());
        info.id
    };
    let _ = events.send(ProcessEvent::StateChanged {
        process_id: id,
        state,
    });
    let _ = events.send(ProcessEvent::Exited {
        process_id: id,
        exit,
    });
    record.finished.notify_waiters();
}

#[cfg(unix)]
fn exit_signal(status: Option<std::process::ExitStatus>) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.and_then(|status| status.signal())
}

#[cfg(not(unix))]
fn exit_signal(_status: Option<std::process::ExitStatus>) -> Option<i32> {
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
