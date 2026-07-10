use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::model::{ArtifactPaths, Error, LaunchRequest, Record, Result, Status, STATE_VERSION};
use crate::store::{load_record, now_ms, save_record, state_path, validate_id, worker_socket_path};

const START_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Executor {
    root: PathBuf,
}

impl Executor {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn launch(&self, request: LaunchRequest) -> Result<Record> {
        validate_id(&request.parent_id)?;
        validate_id(&request.child_id)?;
        if request.prompt.trim().is_empty() {
            return Err(Error::Protocol("launch requires a non-empty prompt".into()));
        }
        let directory = self.child_dir(&request.parent_id, &request.child_id)?;
        if state_path(&directory).exists() {
            return Err(Error::State(format!(
                "subagent `{}` already exists",
                request.child_id
            )));
        }
        fs::create_dir_all(&directory)?;
        let session = directory.join("session.jsonl");
        let record = Record {
            version: STATE_VERSION,
            parent_id: request.parent_id,
            child_id: request.child_id,
            cwd: request.cwd,
            executable: request.executable,
            worker_executable: request.worker_executable,
            child_args: request.child_args,
            environment: request.environment,
            socket: worker_socket_path(&directory)?,
            artifacts: ArtifactPaths {
                state: state_path(&directory),
                transcript: directory.join("transcript.jsonl"),
                stderr: directory.join("child.stderr.log"),
                session,
            },
            status: Status::Pending,
            session_id: format!("subagent-{}", now_ms()),
            turn_id: None,
            summary: None,
            diagnostics: Vec::new(),
            timeout_seconds: request.timeout_seconds,
            metadata: request.metadata,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        save_record(&record)?;
        let mut worker = Command::new(&record.worker_executable);
        worker
            .arg("__imp-subagent-worker")
            .arg(&record.artifacts.state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            worker.process_group(0);
        }
        worker.spawn().map_err(|error| {
            Error::Process(format!("failed to start imp-subagent worker: {error}"))
        })?;
        wait_ready(&record)?;
        request_worker(
            &record,
            Request::Prompt {
                prompt: request.prompt,
            },
        )?;
        self.record(&record.parent_id, &record.child_id)
    }

    pub fn record(&self, parent: &str, child: &str) -> Result<Record> {
        load_record(&state_path(&self.child_dir(parent, child)?))
    }
    pub fn status(&self, parent: &str, child: &str) -> Result<Record> {
        self.record(parent, child)
    }
    pub fn send(&self, parent: &str, child: &str, prompt: String) -> Result<Record> {
        if prompt.trim().is_empty() {
            return Err(Error::Protocol("send requires a non-empty prompt".into()));
        }
        let record = self.record(parent, child)?;
        if record.status == Status::Cancelled {
            return Err(Error::Protocol(
                "cannot send to a cancelled subagent".into(),
            ));
        }
        request_worker(&record, Request::FollowUp { prompt })?;
        self.record(parent, child)
    }
    pub fn cancel(&self, parent: &str, child: &str) -> Result<Record> {
        let record = self.record(parent, child)?;
        if record.status.terminal() {
            return Ok(record);
        }
        request_worker(&record, Request::Stop)?;
        wait_terminal(&record, Duration::from_secs(5))
    }
    pub fn wait(&self, parent: &str, child: &str, timeout: Duration) -> Result<Record> {
        let record = self.record(parent, child)?;
        if timeout.is_zero() {
            return Ok(record);
        }
        wait_terminal(&record, timeout)
    }
    fn child_dir(&self, parent: &str, child: &str) -> Result<PathBuf> {
        validate_id(parent)?;
        validate_id(child)?;
        Ok(self
            .root
            .join(".imp/runs")
            .join(parent)
            .join("subagents")
            .join(child))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum Request {
    Prompt { prompt: String },
    FollowUp { prompt: String },
    Stop,
    Ping,
}
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Response {
    pub(crate) ok: bool,
    pub(crate) turn: Option<String>,
    pub(crate) error: Option<String>,
}

fn wait_ready(record: &Record) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if request_worker(record, Request::Ping).is_ok() {
            return Ok(());
        }
        let current = load_record(&record.artifacts.state)?;
        if current.status.terminal() {
            return Err(Error::Unavailable(current.diagnostics.join("; ")));
        }
        if Instant::now() >= deadline {
            return Err(Error::Unavailable(
                "timed out waiting for imp-subagent worker".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn wait_terminal(record: &Record, timeout: Duration) -> Result<Record> {
    let deadline = Instant::now() + timeout;
    loop {
        let current = load_record(&record.artifacts.state)?;
        if current.status.terminal() {
            return Ok(current);
        }
        if Instant::now() >= deadline {
            return Err(Error::Unavailable(format!(
                "subagent {} did not settle",
                current.child_id
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn request_worker(record: &Record, request: Request) -> Result<Response> {
    #[cfg(unix)]
    {
        request_unix(&record.socket, request)
    }
    #[cfg(not(unix))]
    {
        let _ = record;
        let _ = request;
        Err(Error::Unavailable(
            "imp-subagent workers require Unix sockets on this platform".into(),
        ))
    }
}
#[cfg(unix)]
fn request_unix(socket: &Path, request: Request) -> Result<Response> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| Error::Unavailable(format!("{}: {error}", socket.display())))?;
    serde_json::to_writer(&mut stream, &request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let response: Response = serde_json::from_reader(stream)?;
    if response.ok {
        Ok(response)
    } else {
        Err(Error::Protocol(
            response
                .error
                .unwrap_or_else(|| "worker rejected request".into()),
        ))
    }
}
