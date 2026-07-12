mod drain;
mod error;
mod grant;
mod io;
mod launch;
mod manager;
mod model;
mod observe;
mod one_shot;
mod output;
mod redaction;
mod retention;
mod supervisor;

pub use error::{ProcessError, StopOptions};
pub use grant::{
    EnforcementControl, EnforcementSummary, ExecutionGrant, HostProcessBackend,
    IsolationRequirement, NetworkGrant, ProcessBackend, ProcessRestrictions,
};
pub use manager::ProcessManager;
pub use model::{
    CommandSpec, OutputCursor, OutputStream, ProcessChunk, ProcessEvent, ProcessExit, ProcessId,
    ProcessInfo, ProcessMode, ProcessOutput, ProcessRequest, ProcessSignal, ProcessState,
    SecretEnvironment,
};
pub use one_shot::{state_is_success, OneShotOutcome};

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;
