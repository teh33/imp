use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use super::grant::{EnforcementSummary, ExecutionGrant};

/// Stable runtime identity. It is never an operating-system PID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessId(uuid::Uuid);

impl ProcessId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        uuid::Uuid::parse_str(value).map(Self)
    }
}

impl serde::Serialize for ProcessId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ProcessId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProcessId({})", self.0)
    }
}

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub arguments: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            arguments: Vec::new(),
        }
    }

    pub fn with_arguments(mut self, arguments: impl IntoIterator<Item = String>) -> Self {
        self.arguments = arguments.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessMode {
    Pipes,
    Pty,
}

#[derive(Clone)]
pub struct SecretEnvironment {
    pub secret_id: String,
    pub name: String,
    pub value: String,
}

impl fmt::Debug for SecretEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretEnvironment")
            .field("secret_id", &self.secret_id)
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub struct ProcessRequest {
    pub command: CommandSpec,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub approved_secret_environment: Vec<SecretEnvironment>,
    pub mode: ProcessMode,
    pub timeout: Option<Duration>,
    pub output_retention_bytes: usize,
    pub grant: ExecutionGrant,
}

impl fmt::Debug for ProcessRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessRequest")
            .field("program", &self.command.program)
            .field("argument_count", &self.command.arguments.len())
            .field("cwd", &self.cwd)
            .field("environment_keys", &self.environment.keys().collect::<Vec<_>>())
            .field("approved_secret_environment", &self.approved_secret_environment)
            .field("mode", &self.mode)
            .field("timeout", &self.timeout)
            .field("output_retention_bytes", &self.output_retention_bytes)
            .field("grant", &self.grant)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Running,
    Exited,
    Failed,
    Cancelled,
    TimedOut,
}

impl ProcessState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Cancelled | Self::TimedOut)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub started_at_ms: u64,
    pub exited_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSignal {
    Interrupt,
    Terminate,
    Kill,
    Hangup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OutputCursor {
    pub process_id: ProcessId,
    pub position: u64,
}

impl OutputCursor {
    pub fn start(process_id: ProcessId) -> Self {
        Self { process_id, position: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessChunk {
    pub stream: OutputStream,
    pub start: u64,
    pub end: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessOutput {
    pub process_id: ProcessId,
    pub chunks: Vec<ProcessChunk>,
    pub next_cursor: OutputCursor,
    pub state: ProcessState,
    pub unread_output_evicted: bool,
    pub response_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub id: ProcessId,
    pub program: String,
    pub cwd: PathBuf,
    pub state: ProcessState,
    pub started_at_ms: u64,
    pub exit: Option<ProcessExit>,
    pub next_cursor: OutputCursor,
    pub enforcement: EnforcementSummary,
    pub approved_secret_ids: BTreeSet<String>,
    pub approved_secret_environment: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessEvent {
    Started { process_id: ProcessId },
    OutputAvailable { process_id: ProcessId, next_cursor: OutputCursor },
    StateChanged { process_id: ProcessId, state: ProcessState },
    Exited { process_id: ProcessId, exit: ProcessExit },
}
