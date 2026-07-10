use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use super::manager::ProcessRecord;
use super::{ProcessError, ProcessId};

pub(super) const MAX_RETAINED_PROCESSES: usize = 256;

pub(super) fn prune_terminal_records(
    processes: &Mutex<HashMap<ProcessId, std::sync::Arc<ProcessRecord>>>,
) -> Result<(), ProcessError> {
    let mut processes = lock(processes);
    if processes.len() < MAX_RETAINED_PROCESSES {
        return Ok(());
    }
    let excess = processes.len() - MAX_RETAINED_PROCESSES + 1;
    let mut terminal = processes
        .iter()
        .filter_map(|(id, record)| {
            let info = lock(&record.info);
            info.state
                .is_terminal()
                .then_some((*id, info.started_at_ms))
        })
        .collect::<Vec<_>>();
    if terminal.len() < excess {
        return Err(ProcessError::CapacityExceeded(MAX_RETAINED_PROCESSES));
    }
    terminal.sort_by_key(|(_, started_at)| *started_at);
    for (id, _) in terminal.into_iter().take(excess) {
        processes.remove(&id);
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
