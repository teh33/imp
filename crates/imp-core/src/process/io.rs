use tokio::io::AsyncWriteExt;

use super::{ProcessError, ProcessId, ProcessManager};
use crate::process::manager::lock;

impl ProcessManager {
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

    pub async fn close_stdin(&self, id: ProcessId) -> Result<(), ProcessError> {
        let record = self.record(id)?;
        record.stdin.lock().await.take();
        Ok(())
    }
}
