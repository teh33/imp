use std::time::Duration;

use tokio::sync::broadcast;

use super::{OutputCursor, ProcessError, ProcessEvent, ProcessId, ProcessManager, ProcessOutput};

impl ProcessManager {
    pub async fn observe(
        &self,
        id: ProcessId,
        cursor: OutputCursor,
        wait: Duration,
        max_bytes: usize,
    ) -> Result<ProcessOutput, ProcessError> {
        let mut events = self.subscribe();
        let initial = self.read(id, cursor, max_bytes).await?;
        if !initial.chunks.is_empty() || initial.state.is_terminal() || wait.is_zero() {
            return Ok(initial);
        }
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Ok(event)) if event_process_id(&event) == id => {
                    let output = self.read(id, cursor, max_bytes).await?;
                    if !output.chunks.is_empty() || output.state.is_terminal() {
                        return Ok(output);
                    }
                }
                Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => {
                    return self.read(id, cursor, max_bytes).await;
                }
            }
        }
    }
}

fn event_process_id(event: &ProcessEvent) -> ProcessId {
    match event {
        ProcessEvent::Started { process_id }
        | ProcessEvent::OutputAvailable { process_id, .. }
        | ProcessEvent::StateChanged { process_id, .. }
        | ProcessEvent::Exited { process_id, .. } => *process_id,
    }
}
