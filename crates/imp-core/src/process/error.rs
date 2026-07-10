use std::time::Duration;

use thiserror::Error;

use super::ProcessId;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("unknown process {0}")]
    UnknownProcess(ProcessId),
    #[error("unsupported process capability: {0}")]
    UnsupportedCapability(&'static str),
    #[error("required process isolation is unavailable")]
    IsolationUnavailable,
    #[error("execution grant denied: {0}")]
    GrantDenied(String),
    #[error("process {0} is no longer running")]
    NotRunning(ProcessId),
    #[error("output cursor belongs to a different process")]
    CursorProcessMismatch,
    #[error("failed to spawn process: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("process manager capacity of {0} active processes is exhausted")]
    CapacityExceeded(usize),
    #[error("process supervisor stopped unexpectedly")]
    SupervisorStopped,
}

#[derive(Debug, Clone, Copy)]
pub struct StopOptions {
    pub grace: Duration,
}

impl Default for StopOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_millis(500),
        }
    }
}
