use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{broadcast, mpsc, Notify};

use crate::child_process::{isolate_tokio_command, signal_process_group, ProcessGroupSignal};

use super::drain::spawn_drain;
use super::launch::{build_command, process_info};
use super::output::OutputBuffer;
use super::retention::prune_terminal_records;
use super::supervisor::spawn_supervisor;
use super::{
    HostProcessBackend, OutputCursor, OutputStream, ProcessBackend, ProcessError, ProcessEvent,
    ProcessExit, ProcessId, ProcessInfo, ProcessOutput, ProcessRequest, ProcessSignal,
    ProcessState, StopOptions,
};

const DEFAULT_RETENTION_BYTES: usize = 64 * 1024;
const MAX_RETENTION_BYTES: usize = 4 * 1024 * 1024;
const MAX_READ_BYTES: usize = 64 * 1024;
const EVENT_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    backend: Arc<dyn ProcessBackend>,
    processes: Mutex<HashMap<ProcessId, Arc<ProcessRecord>>>,
    start_lock: tokio::sync::Mutex<()>,
    events: broadcast::Sender<ProcessEvent>,
}

pub(super) struct ProcessRecord {
    pub(super) info: Mutex<ProcessInfo>,
    pub(super) output: Mutex<OutputBuffer>,
    pub(super) output_redactions: Arc<Vec<Vec<u8>>>,
    os_pid: Option<u32>,
    pub(super) stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    control: mpsc::UnboundedSender<Control>,
    pub(super) finished: Notify,
}

#[derive(Debug)]
pub(super) enum Control {
    Signal(ProcessSignal),
    Stop(StopOptions),
    Cancel,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self::with_backend(Arc::new(HostProcessBackend))
    }

    pub fn with_backend(backend: Arc<dyn ProcessBackend>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            inner: Arc::new(ManagerInner {
                backend,
                processes: Mutex::new(HashMap::new()),
                start_lock: tokio::sync::Mutex::new(()),
                events,
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.inner.events.subscribe()
    }

    pub async fn start(&self, mut request: ProcessRequest) -> Result<ProcessInfo, ProcessError> {
        let _start = self.inner.start_lock.lock().await;
        prune_terminal_records(&self.inner.processes)?;
        if request.output_retention_bytes == 0 {
            request.output_retention_bytes = DEFAULT_RETENTION_BYTES;
        }
        request.output_retention_bytes = request.output_retention_bytes.min(MAX_RETENTION_BYTES);
        let enforcement = self.inner.backend.validate(&request)?;
        let id = ProcessId::new();
        let started_at_ms = now_ms();
        let mut command = build_command(&request);
        isolate_tokio_command(&mut command);
        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        let os_pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::other("stdout pipe was not created"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::other("stderr pipe was not created"))
        })?;
        let (control, control_rx) = mpsc::unbounded_channel();
        let info = process_info(id, &request, enforcement, started_at_ms);
        let record = Arc::new(ProcessRecord {
            info: Mutex::new(info.clone()),
            output: Mutex::new(OutputBuffer::new(id, request.output_retention_bytes)),
            output_redactions: Arc::new(
                request
                    .approved_secret_environment
                    .iter()
                    .filter(|secret| !secret.value.is_empty())
                    .map(|secret| secret.value.as_bytes().to_vec())
                    .collect(),
            ),
            os_pid,
            stdin: tokio::sync::Mutex::new(stdin),
            control,
            finished: Notify::new(),
        });
        lock(&self.inner.processes).insert(id, Arc::clone(&record));
        let _ = self
            .inner
            .events
            .send(ProcessEvent::Started { process_id: id });
        set_state(&self.inner.events, &record, ProcessState::Running);

        let stdout_task = spawn_drain(
            stdout,
            OutputStream::Stdout,
            Arc::clone(&record),
            self.inner.events.clone(),
        );
        let stderr_task = spawn_drain(
            stderr,
            OutputStream::Stderr,
            Arc::clone(&record),
            self.inner.events.clone(),
        );
        spawn_supervisor(
            child,
            os_pid,
            request.timeout,
            control_rx,
            [stdout_task, stderr_task],
            Arc::clone(&record),
            self.inner.events.clone(),
            started_at_ms,
        );
        let started_info = lock(&record.info).clone();
        Ok(started_info)
    }

    pub async fn read(
        &self,
        id: ProcessId,
        cursor: OutputCursor,
        max_bytes: usize,
    ) -> Result<ProcessOutput, ProcessError> {
        if cursor.process_id != id {
            return Err(ProcessError::CursorProcessMismatch);
        }
        let record = self.record(id)?;
        let read = lock(&record.output).read(cursor.position, max_bytes.min(MAX_READ_BYTES));
        let state = lock(&record.info).state;
        Ok(ProcessOutput {
            process_id: id,
            chunks: read.chunks,
            next_cursor: OutputCursor {
                process_id: id,
                position: read.next_position,
            },
            state,
            unread_output_evicted: read.evicted,
            response_truncated: read.truncated,
        })
    }

    pub async fn write(&self, id: ProcessId, bytes: &[u8]) -> Result<usize, ProcessError> {
        let record = self.record(id)?;
        if lock(&record.info).state.is_terminal() {
            return Err(ProcessError::NotRunning(id));
        }
        let mut stdin = record.stdin.lock().await;
        let writer = stdin.as_mut().ok_or(ProcessError::NotRunning(id))?;
        writer.write_all(bytes).await?;
        writer.flush().await?;
        Ok(bytes.len())
    }

    pub async fn signal(&self, id: ProcessId, signal: ProcessSignal) -> Result<(), ProcessError> {
        let record = self.record(id)?;
        if lock(&record.info).state.is_terminal() {
            return Err(ProcessError::NotRunning(id));
        }
        record
            .control
            .send(Control::Signal(signal))
            .map_err(|_| ProcessError::SupervisorStopped)
    }

    pub async fn stop(
        &self,
        id: ProcessId,
        options: StopOptions,
    ) -> Result<ProcessExit, ProcessError> {
        let record = self.record(id)?;
        if let Some(exit) = lock(&record.info).exit.clone() {
            return Ok(exit);
        }
        record
            .control
            .send(Control::Stop(options))
            .map_err(|_| ProcessError::SupervisorStopped)?;
        self.wait(id).await
    }

    pub async fn cancel(&self, id: ProcessId) -> Result<ProcessExit, ProcessError> {
        let record = self.record(id)?;
        if let Some(exit) = lock(&record.info).exit.clone() {
            return Ok(exit);
        }
        record
            .control
            .send(Control::Cancel)
            .map_err(|_| ProcessError::SupervisorStopped)?;
        self.wait(id).await
    }

    pub async fn wait(&self, id: ProcessId) -> Result<ProcessExit, ProcessError> {
        let record = self.record(id)?;
        loop {
            let notified = record.finished.notified();
            if let Some(exit) = lock(&record.info).exit.clone() {
                return Ok(exit);
            }
            notified.await;
        }
    }

    pub fn get(&self, id: ProcessId) -> Result<ProcessInfo, ProcessError> {
        self.record(id).map(|record| lock(&record.info).clone())
    }

    pub fn list(&self) -> Vec<ProcessInfo> {
        let mut items = lock(&self.inner.processes)
            .values()
            .map(|record| lock(&record.info).clone())
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.started_at_ms);
        items
    }

    fn record(&self, id: ProcessId) -> Result<Arc<ProcessRecord>, ProcessError> {
        lock(&self.inner.processes)
            .get(&id)
            .cloned()
            .ok_or(ProcessError::UnknownProcess(id))
    }
}

impl Drop for ManagerInner {
    fn drop(&mut self) {
        for record in lock(&self.processes).values() {
            if !lock(&record.info).state.is_terminal() {
                if let Some(pid) = record.os_pid {
                    signal_process_group(pid, ProcessGroupSignal::Kill);
                }
                let _ = record.control.send(Control::Cancel);
            }
        }
    }
}

fn set_state(
    events: &broadcast::Sender<ProcessEvent>,
    record: &ProcessRecord,
    state: ProcessState,
) {
    let id = {
        let mut info = lock(&record.info);
        info.state = state;
        info.id
    };
    let _ = events.send(ProcessEvent::StateChanged {
        process_id: id,
        state,
    });
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
