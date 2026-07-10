use std::collections::{BTreeSet, HashMap};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, Notify};

use crate::child_process::{isolate_tokio_command, signal_process_group, ProcessGroupSignal};

use super::output::OutputBuffer;
use super::{
    EnforcementSummary, HostProcessBackend, OutputCursor, OutputStream, ProcessBackend,
    ProcessError, ProcessEvent, ProcessExit, ProcessId, ProcessInfo, ProcessOutput, ProcessRequest,
    ProcessSignal, ProcessState, StopOptions,
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
    events: broadcast::Sender<ProcessEvent>,
}

struct ProcessRecord {
    info: Mutex<ProcessInfo>,
    output: Mutex<OutputBuffer>,
    output_redactions: Arc<Vec<Vec<u8>>>,
    os_pid: Option<u32>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    control: mpsc::UnboundedSender<Control>,
    finished: Notify,
}

#[derive(Debug)]
enum Control {
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
                events,
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.inner.events.subscribe()
    }

    pub async fn start(&self, mut request: ProcessRequest) -> Result<ProcessInfo, ProcessError> {
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
        let _ = self.inner.events.send(ProcessEvent::Started { process_id: id });
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
            next_cursor: OutputCursor { process_id: id, position: read.next_position },
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
        record.control.send(Control::Signal(signal)).map_err(|_| ProcessError::SupervisorStopped)
    }

    pub async fn stop(&self, id: ProcessId, options: StopOptions) -> Result<ProcessExit, ProcessError> {
        let record = self.record(id)?;
        if let Some(exit) = lock(&record.info).exit.clone() {
            return Ok(exit);
        }
        record.control.send(Control::Stop(options)).map_err(|_| ProcessError::SupervisorStopped)?;
        self.wait(id).await
    }

    pub async fn cancel(&self, id: ProcessId) -> Result<ProcessExit, ProcessError> {
        let record = self.record(id)?;
        if let Some(exit) = lock(&record.info).exit.clone() {
            return Ok(exit);
        }
        record.control.send(Control::Cancel).map_err(|_| ProcessError::SupervisorStopped)?;
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
        lock(&self.inner.processes).get(&id).cloned().ok_or(ProcessError::UnknownProcess(id))
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
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

fn build_command(request: &ProcessRequest) -> Command {
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

fn process_info(
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

fn spawn_drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    stream: OutputStream,
    record: Arc<ProcessRecord>,
    events: broadcast::Sender<ProcessEvent>,
) -> tokio::task::JoinHandle<()> {
    let redactions = Arc::clone(&record.output_redactions);
    tokio::spawn(async move {
        let mut bytes = vec![0; 8192];
        let mut redactor = StreamRedactor::new(redactions);
        loop {
            let Ok(count) = reader.read(&mut bytes).await else {
                break;
            };
            if count == 0 {
                publish_output(&record, &events, stream, &redactor.finish());
                break;
            }
            publish_output(
                &record,
                &events,
                stream,
                &redactor.push(&bytes[..count]),
            );
        }
    })
}

fn publish_output(
    record: &ProcessRecord,
    events: &broadcast::Sender<ProcessEvent>,
    stream: OutputStream,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    let next = {
        let mut output = lock(&record.output);
        output.push(stream, bytes);
        output.next_position()
    };
    let process_id = {
        let mut info = lock(&record.info);
        info.next_cursor.position = next;
        info.id
    };
    let _ = events.send(ProcessEvent::OutputAvailable {
        process_id,
        next_cursor: OutputCursor {
            process_id,
            position: next,
        },
    });
}

struct StreamRedactor {
    secrets: Arc<Vec<Vec<u8>>>,
    pending: Vec<u8>,
    hold_back: usize,
}

impl StreamRedactor {
    fn new(secrets: Arc<Vec<Vec<u8>>>) -> Self {
        let hold_back = secrets.iter().map(Vec::len).max().unwrap_or(1).saturating_sub(1);
        Self {
            secrets,
            pending: Vec::new(),
            hold_back,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let proposed = self.pending.len().saturating_sub(self.hold_back);
        let emit_len = self.secrets.iter().fold(proposed, |boundary, secret| {
            crossing_match_start(&self.pending, secret, boundary).unwrap_or(boundary)
        });
        if emit_len == 0 {
            return Vec::new();
        }
        let emitted = self.pending.drain(..emit_len).collect::<Vec<_>>();
        redact_bytes(emitted, &self.secrets)
    }

    fn finish(mut self) -> Vec<u8> {
        redact_bytes(std::mem::take(&mut self.pending), &self.secrets)
    }
}

fn redact_bytes(mut bytes: Vec<u8>, secrets: &[Vec<u8>]) -> Vec<u8> {
    const REDACTION: &[u8] = b"[REDACTED_SECRET]";
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        let mut redacted = Vec::with_capacity(bytes.len());
        let mut offset = 0;
        while let Some(found) = find_bytes(&bytes[offset..], secret) {
            let start = offset + found;
            redacted.extend_from_slice(&bytes[offset..start]);
            redacted.extend_from_slice(REDACTION);
            offset = start + secret.len();
        }
        redacted.extend_from_slice(&bytes[offset..]);
        bytes = redacted;
    }
    bytes
}

fn crossing_match_start(haystack: &[u8], needle: &[u8], boundary: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .filter(|start| *start < boundary && start + needle.len() > boundary)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn spawn_supervisor(
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
    let _ = events.send(ProcessEvent::StateChanged { process_id: id, state });
    let _ = events.send(ProcessEvent::Exited { process_id: id, exit });
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

fn set_state(events: &broadcast::Sender<ProcessEvent>, record: &ProcessRecord, state: ProcessState) {
    let id = {
        let mut info = lock(&record.info);
        info.state = state;
        info.id
    };
    let _ = events.send(ProcessEvent::StateChanged { process_id: id, state });
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_redactor_hides_secret_split_across_reads() {
        let secrets = Arc::new(vec![b"private-token".to_vec()]);
        let mut redactor = StreamRedactor::new(secrets);
        let mut output = redactor.push(b"before private-");
        output.extend(redactor.push(b"token after"));
        output.extend(redactor.finish());
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text, "before [REDACTED_SECRET] after");
        assert!(!text.contains("private-token"));
    }
}
