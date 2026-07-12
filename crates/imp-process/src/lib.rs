use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to {operation} process: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("process did not exit within {0:?}")]
    Timeout(Duration),
    #[error("process {0} stream was not piped")]
    MissingPipe(&'static str),
    #[error("process id exceeds the platform pid range")]
    InvalidPid,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    Inherit,
    Null,
    Piped,
}

#[derive(Debug)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub stdin: StreamMode,
    pub stdout: StreamMode,
    pub stderr: StreamMode,
    pub stderr_file: Option<PathBuf>,
    pub process_group: bool,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            environment: Vec::new(),
            cwd: None,
            stdin: StreamMode::Null,
            stdout: StreamMode::Null,
            stderr: StreamMode::Null,
            stderr_file: None,
            process_group: true,
        }
    }
}

#[derive(Debug)]
pub struct ManagedChild {
    child: Child,
    process_group: bool,
}

impl ManagedChild {
    pub fn spawn(spec: &CommandSpec) -> Result<Self> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.environment.iter().map(|(key, value)| (key, value)))
            .stdin(stdio(spec.stdin))
            .stdout(stdio(spec.stdout));
        if let Some(path) = &spec.stderr_file {
            let file = std::fs::File::create(path).map_err(|source| Error::Io {
                operation: "open stderr capture for",
                source,
            })?;
            command.stderr(Stdio::from(file));
        } else {
            command.stderr(stdio(spec.stderr));
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        #[cfg(unix)]
        if spec.process_group {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().map_err(|source| Error::Io {
            operation: "spawn",
            source,
        })?;
        Ok(Self {
            child,
            process_group: spec.process_group,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn take_stdin(&mut self) -> Result<ChildStdin> {
        self.child.stdin.take().ok_or(Error::MissingPipe("stdin"))
    }

    pub fn take_stdout(&mut self) -> Result<ChildStdout> {
        self.child.stdout.take().ok_or(Error::MissingPipe("stdout"))
    }

    pub fn take_stderr(&mut self) -> Result<ChildStderr> {
        self.child.stderr.take().ok_or(Error::MissingPipe("stderr"))
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().map_err(|source| Error::Io {
            operation: "observe",
            source,
        })
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child.wait().map_err(|source| Error::Io {
            operation: "wait for",
            source,
        })
    }

    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn terminate(&mut self, grace: Duration) -> Result<ExitStatus> {
        if let Some(status) = self.try_wait()? {
            return Ok(status);
        }
        self.signal_terminate()?;
        match self.wait_timeout(grace) {
            Ok(status) => Ok(status),
            Err(Error::Timeout(_)) => {
                self.signal_kill()?;
                self.wait()
            }
            Err(error) => Err(error),
        }
    }

    fn signal_terminate(&mut self) -> Result<()> {
        self.signal(libc_signal_terminate(), "terminate")
    }

    fn signal_kill(&mut self) -> Result<()> {
        self.signal(libc_signal_kill(), "kill")
    }

    fn signal(&mut self, signal: i32, operation: &'static str) -> Result<()> {
        #[cfg(unix)]
        if self.process_group {
            let pid = i32::try_from(self.id()).map_err(|_| Error::InvalidPid)?;
            // SAFETY: spawn creates a process group whose id is the child pid.
            let result = unsafe { libc::kill(-pid, signal) };
            if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(Error::Io {
                operation,
                source: std::io::Error::last_os_error(),
            });
        }
        self.child
            .kill()
            .map_err(|source| Error::Io { operation, source })
    }
}

#[cfg(unix)]
const fn libc_signal_terminate() -> i32 {
    libc::SIGTERM
}
#[cfg(not(unix))]
const fn libc_signal_terminate() -> i32 {
    0
}
#[cfg(unix)]
const fn libc_signal_kill() -> i32 {
    libc::SIGKILL
}
#[cfg(not(unix))]
const fn libc_signal_kill() -> i32 {
    0
}

fn stdio(mode: StreamMode) -> Stdio {
    match mode {
        StreamMode::Inherit => Stdio::inherit(),
        StreamMode::Null => Stdio::null(),
        StreamMode::Piped => Stdio::piped(),
    }
}

pub fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_piped_stdout_and_observes_exit() {
        let mut spec = CommandSpec::new("sh");
        spec.args = vec!["-c".into(), "printf ready".into()];
        spec.stdout = StreamMode::Piped;
        let mut child = ManagedChild::spawn(&spec).unwrap();
        let mut output = String::new();
        use std::io::Read;
        child
            .take_stdout()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(output, "ready");
    }

    #[test]
    fn timeout_is_typed() {
        let mut spec = CommandSpec::new("sh");
        spec.args = vec!["-c".into(), "sleep 1".into()];
        let mut child = ManagedChild::spawn(&spec).unwrap();
        assert!(matches!(
            child.wait_timeout(Duration::from_millis(10)),
            Err(Error::Timeout(_))
        ));
        child.terminate(Duration::from_millis(100)).unwrap();
    }
}
