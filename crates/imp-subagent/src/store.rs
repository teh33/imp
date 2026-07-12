use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;

use crate::model::{Error, Record, Result, STATE_VERSION};

pub(crate) fn timed_out(record: &Record) -> bool {
    record.timeout_seconds.is_some_and(|timeout| {
        now_ms().saturating_sub(record.created_at_ms) >= u128::from(timeout) * 1_000
    })
}

pub(crate) fn worker_socket_path(directory: &Path) -> Result<PathBuf> {
    let mut hasher = DefaultHasher::new();
    directory.hash(&mut hasher);
    let root = std::env::temp_dir().join("imp-subagent");
    fs::create_dir_all(&root)?;
    Ok(root.join(format!("{:016x}.sock", hasher.finish())))
}

pub(crate) fn state_path(directory: &Path) -> PathBuf {
    directory.join("state.json")
}
pub(crate) fn save_record(record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record)?;
    let parent = record
        .artifacts
        .state
        .parent()
        .ok_or_else(|| Error::State("state path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join("state.lock"))?;
    lock.lock_exclusive()?;
    let temporary = parent.join(format!(".state-{}.tmp", now_ms()));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, &record.artifacts.state)?;
    Ok(())
}
pub(crate) fn load_record(path: &Path) -> Result<Record> {
    match fs::read(path) {
        Ok(bytes) => {
            let record: Record = serde_json::from_slice(&bytes)?;
            if record.version != STATE_VERSION {
                return Err(Error::State(format!(
                    "unsupported subagent state version {}",
                    record.version
                )));
            }
            Ok(record)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::NotFound(path.display().to_string()))
        }
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(Error::InvalidId(id.into()))
    } else {
        Ok(())
    }
}
pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
