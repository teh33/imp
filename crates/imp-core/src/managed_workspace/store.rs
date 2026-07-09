use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::{ManagedWorkspaceError, ManagedWorkspaceRegistry, ManagedWorkspaceResult};

const LOCK_WAIT: Duration = Duration::from_secs(2);
const STALE_LOCK_AGE: Duration = Duration::from_secs(60);

pub(super) struct RegistryStore {
    root: PathBuf,
}

impl RegistryStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn repo_dir(&self, repo_root: &Path) -> PathBuf {
        self.root.join(repo_key(repo_root))
    }

    pub fn registry_path(&self, repo_root: &Path) -> PathBuf {
        self.repo_dir(repo_root).join("registry.json")
    }

    pub fn worktree_root(&self, repo_root: &Path) -> PathBuf {
        self.repo_dir(repo_root).join("trees")
    }

    pub fn update<T>(
        &self,
        repo_root: &Path,
        operation: impl FnOnce(&mut ManagedWorkspaceRegistry) -> ManagedWorkspaceResult<T>,
    ) -> ManagedWorkspaceResult<T> {
        let repo_dir = self.repo_dir(repo_root);
        fs::create_dir_all(&repo_dir)?;
        let _lock = RegistryLock::acquire(repo_dir.join("registry.lock"))?;
        let mut registry = self.load(repo_root)?;
        let output = operation(&mut registry)?;
        self.write(repo_root, &registry)?;
        Ok(output)
    }

    pub fn load(&self, repo_root: &Path) -> ManagedWorkspaceResult<ManagedWorkspaceRegistry> {
        let path = self.registry_path(repo_root);
        if !path.exists() {
            return Ok(ManagedWorkspaceRegistry::empty(repo_root.to_path_buf()));
        }
        let bytes = fs::read(&path)?;
        let registry: ManagedWorkspaceRegistry = serde_json::from_slice(&bytes)?;
        if registry.schema_version != 1 || registry.repo_root != repo_root {
            return Err(ManagedWorkspaceError::Registry(format!(
                "registry identity mismatch: {}",
                path.display()
            )));
        }
        Ok(registry)
    }

    fn write(
        &self,
        repo_root: &Path,
        registry: &ManagedWorkspaceRegistry,
    ) -> ManagedWorkspaceResult<()> {
        let path = self.registry_path(repo_root);
        let temp = path.with_extension(format!("tmp-{}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(registry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &path)?;
        Ok(())
    }
}

struct RegistryLock {
    path: PathBuf,
}

impl RegistryLock {
    fn acquire(path: PathBuf) -> ManagedWorkspaceResult<Self> {
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&path)? {
                        fs::remove_file(&path)?;
                        continue;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err(ManagedWorkspaceError::Registry(format!(
                            "workspace registry is busy: {}",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> ManagedWorkspaceResult<bool> {
    let modified = fs::metadata(path)?.modified()?;
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        >= STALE_LOCK_AGE)
}

fn repo_key(repo_root: &Path) -> String {
    let canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let name = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("repo")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    format!("{name}-{hash:016x}")
}
