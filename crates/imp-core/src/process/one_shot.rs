use std::time::Duration;

use super::{
    OutputCursor, ProcessError, ProcessExit, ProcessId, ProcessManager, ProcessOutput,
    ProcessRequest, ProcessState,
};

#[derive(Debug, Clone)]
pub struct OneShotOutcome {
    pub process_id: ProcessId,
    pub exit: ProcessExit,
    pub output: ProcessOutput,
}

impl ProcessManager {
    pub async fn run_one_shot(
        &self,
        request: ProcessRequest,
        cancellation: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<OneShotOutcome, ProcessError> {
        let info = self.start(request).await?;
        let id = info.id;
        let exit = loop {
            if cancellation.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                break self.cancel(id).await?;
            }
            let state = self.get(id)?.state;
            if state.is_terminal() {
                break self.wait(id).await?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let output = self.read(id, OutputCursor::start(id), usize::MAX).await?;
        Ok(OneShotOutcome {
            process_id: id,
            exit,
            output,
        })
    }

    pub async fn collect_until_terminal(
        &self,
        id: ProcessId,
        cursor: &mut OutputCursor,
    ) -> Result<ProcessOutput, ProcessError> {
        loop {
            let output = self.read(id, *cursor, usize::MAX).await?;
            *cursor = output.next_cursor;
            if output.state.is_terminal() || !output.chunks.is_empty() {
                return Ok(output);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

pub fn state_is_success(state: ProcessState, exit: &ProcessExit) -> bool {
    state == ProcessState::Exited && exit.code == Some(0)
}
