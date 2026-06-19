use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const IMP_DIR_NAME: &str = ".imp";
const LEGACY_APP_NAME: &str = "imp";
const WORK_DIR_NAME: &str = "work";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStoreSource {
    ProjectLocal,
    GlobalProjectScoped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkScope {
    pub project_root: PathBuf,
    pub local_store_root: PathBuf,
    pub global_store_root: PathBuf,
    pub active_source: WorkStoreSource,
    pub writes_target: WorkStoreSource,
}

impl WorkScope {
    pub fn for_project_dir(project_dir: &Path) -> Self {
        Self::with_global_root(project_dir, global_root())
    }

    pub fn with_global_root(project_dir: &Path, global_root: PathBuf) -> Self {
        let project_root = canonicalize_lossy(project_dir);
        Self {
            local_store_root: project_root.join(IMP_DIR_NAME).join(WORK_DIR_NAME),
            global_store_root: global_root.join(WORK_DIR_NAME),
            project_root,
            active_source: WorkStoreSource::GlobalProjectScoped,
            writes_target: WorkStoreSource::GlobalProjectScoped,
        }
    }

    pub fn migration_status(&self) -> &'static str {
        "global project-scoped store is the normal imp-work backend; project-local stores are migration input only"
    }
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn global_root() -> PathBuf {
    global_root_from_env(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

fn global_root_from_env(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> PathBuf {
    home.or(userprofile)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(IMP_DIR_NAME)
}

pub fn project_root(project_dir: &Path) -> PathBuf {
    project_dir.join(IMP_DIR_NAME)
}

pub fn global_config_path() -> PathBuf {
    global_root().join("config.toml")
}

pub fn global_auth_path() -> PathBuf {
    global_root().join("auth.json")
}

pub fn global_agents_path() -> PathBuf {
    global_root().join("agents.md")
}

pub fn global_sessions_dir() -> PathBuf {
    global_root().join("sessions")
}

pub fn global_run_index_path() -> PathBuf {
    global_runs_dir().join("index.jsonl")
}

pub fn global_indexes_dir() -> PathBuf {
    global_root().join("indexes")
}

pub fn global_session_index_path() -> PathBuf {
    global_indexes_dir().join("session_index.db")
}

pub fn global_code_index_path() -> PathBuf {
    global_indexes_dir().join("code-index").join("index.sqlite")
}

pub fn global_skills_dir() -> PathBuf {
    global_root().join("skills")
}

pub fn global_prompts_dir() -> PathBuf {
    global_root().join("prompts")
}

pub fn global_tools_dir() -> PathBuf {
    global_root().join("tools")
}

pub fn global_lua_dir() -> PathBuf {
    global_root().join("lua")
}

pub fn global_imports_dir() -> PathBuf {
    global_root().join("imports")
}

pub fn project_config_path(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("config.toml")
}

pub fn project_agents_path(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("agents.md")
}

pub fn project_skills_dir(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("skills")
}

pub fn project_prompts_dir(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("prompts")
}

pub fn project_tools_dir(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("tools")
}

pub fn project_lua_dir(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("lua")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifacts {
    root: PathBuf,
}

impl RunArtifacts {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn create(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workflow_controller_path(&self) -> PathBuf {
        self.root.join("workflow-controller.json")
    }

    pub fn workflow_contract_path(&self) -> PathBuf {
        self.root.join("workflow-contract.json")
    }

    pub fn trace_path(&self) -> PathBuf {
        self.root.join("trace.jsonl")
    }

    pub fn evidence_path(&self) -> PathBuf {
        self.root.join("evidence.md")
    }

    pub fn diff_path(&self) -> PathBuf {
        self.root.join("diff.patch")
    }

    pub fn verify_log_path(&self) -> PathBuf {
        self.root.join("verify.log")
    }

    pub fn policy_log_path(&self) -> PathBuf {
        self.root.join("policy.jsonl")
    }

    pub fn eval_candidate_path(&self, candidate_id: &str) -> PathBuf {
        self.root
            .join("eval-candidates")
            .join(candidate_id)
            .join("candidate.json")
    }
}

pub fn project_runs_dir(project_dir: &Path) -> PathBuf {
    project_root(project_dir).join("runs")
}

pub fn global_runs_dir() -> PathBuf {
    global_root().join("runs")
}

pub fn project_run_artifacts(project_dir: &Path, run_id: &str) -> io::Result<RunArtifacts> {
    run_artifacts_under(project_runs_dir(project_dir), run_id)
}

pub fn global_run_artifacts(run_id: &str) -> io::Result<RunArtifacts> {
    run_artifacts_under(global_runs_dir(), run_id)
}

pub fn run_artifacts_under(base: PathBuf, run_id: &str) -> io::Result<RunArtifacts> {
    let safe_run_id = sanitize_run_id(run_id)?;
    let root = base.join(safe_run_id);
    ensure_child_path(&base, &root)?;
    RunArtifacts::create(root)
}

pub fn existing_project_run_artifacts(
    project_dir: &Path,
    run_id: &str,
) -> io::Result<RunArtifacts> {
    existing_run_artifacts_under(project_runs_dir(project_dir), run_id)
}

pub fn existing_run_artifacts_under(base: PathBuf, run_id: &str) -> io::Result<RunArtifacts> {
    let safe_run_id = sanitize_run_id(run_id)?;
    let root = base.join(safe_run_id);
    ensure_child_path(&base, &root)?;
    if root.is_dir() {
        Ok(RunArtifacts::new(root))
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "run artifact directory does not exist",
        ))
    }
}

fn sanitize_run_id(run_id: &str) -> io::Result<&str> {
    let valid = !run_id.is_empty()
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(run_id)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "run id must contain only ascii letters, numbers, '-' or '_'",
        ))
    }
}

fn ensure_child_path(base: &Path, child: &Path) -> io::Result<()> {
    if child
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "run artifact path must not contain parent components",
        ));
    }
    if !child.starts_with(base) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "run artifact path escapes base directory",
        ));
    }
    Ok(())
}

pub fn legacy_config_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = xdg_config_root() {
        roots.push(root);
    }
    dedupe(roots)
}

pub fn legacy_data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = xdg_data_root() {
        roots.push(root);
    }
    if cfg!(target_os = "macos") {
        if let Some(root) = macos_application_support_root() {
            roots.push(root);
        }
    }
    dedupe(roots)
}

pub fn global_config_roots_for_read() -> Vec<PathBuf> {
    let mut roots = vec![global_root()];
    roots.extend(legacy_config_roots());
    dedupe(roots)
}

pub fn global_data_roots_for_read() -> Vec<PathBuf> {
    let mut roots = vec![global_root()];
    roots.extend(legacy_data_roots());
    dedupe(roots)
}

pub fn existing_global_file(path_fn: fn() -> PathBuf, legacy_subpath: &str) -> Option<PathBuf> {
    let canonical = path_fn();
    if canonical.exists() {
        return Some(canonical);
    }

    for root in global_config_roots_for_read() {
        let path = root.join(legacy_subpath);
        if path.exists() {
            return Some(path);
        }
    }

    for root in global_data_roots_for_read() {
        let path = root.join(legacy_subpath);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

pub fn existing_global_auth_path() -> Option<PathBuf> {
    let canonical = global_auth_path();
    if canonical.exists() {
        return Some(canonical);
    }
    legacy_config_roots()
        .into_iter()
        .map(|root| root.join("auth.json"))
        .find(|path| path.exists())
}

pub fn existing_global_config_path() -> Option<PathBuf> {
    let canonical = global_config_path();
    if canonical.exists() {
        return Some(canonical);
    }
    legacy_config_roots()
        .into_iter()
        .map(|root| root.join("config.toml"))
        .find(|path| path.exists())
}

pub fn reconcile_legacy_into_global_root() -> io::Result<Vec<PathBuf>> {
    let mut migrated = Vec::new();

    migrated.extend(reconcile_file_candidates(
        global_config_path(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("config.toml"))
            .collect(),
    )?);
    migrated.extend(reconcile_file_candidates(
        global_auth_path(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("auth.json"))
            .collect(),
    )?);
    migrated.extend(reconcile_file_candidates(
        global_agents_path(),
        legacy_config_roots()
            .into_iter()
            .flat_map(|root| {
                [
                    root.join("agents.md"),
                    root.join("AGENTS.md"),
                    root.join("CLAUDE.md"),
                ]
            })
            .collect(),
    )?);
    migrated.extend(reconcile_dir_candidates(
        global_skills_dir(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("skills"))
            .collect(),
    )?);
    migrated.extend(reconcile_dir_candidates(
        global_prompts_dir(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("prompts"))
            .collect(),
    )?);
    migrated.extend(reconcile_dir_candidates(
        global_tools_dir(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("tools"))
            .collect(),
    )?);
    migrated.extend(reconcile_dir_candidates(
        global_lua_dir(),
        legacy_config_roots()
            .into_iter()
            .map(|root| root.join("lua"))
            .collect(),
    )?);
    migrated.extend(reconcile_dir_candidates(
        global_sessions_dir(),
        legacy_data_roots()
            .into_iter()
            .map(|root| root.join("sessions"))
            .collect(),
    )?);
    migrated.extend(reconcile_file_candidates(
        global_session_index_path(),
        legacy_data_roots()
            .into_iter()
            .flat_map(|root| {
                [
                    root.join("indexes").join("session_index.db"),
                    root.join("session_index.db"),
                ]
            })
            .collect(),
    )?);

    Ok(migrated)
}

fn reconcile_file_candidates(
    target: PathBuf,
    candidates: Vec<PathBuf>,
) -> io::Result<Vec<PathBuf>> {
    if target.exists() {
        return Ok(Vec::new());
    }

    let Some(source) = candidates.into_iter().find(|path| path.exists()) else {
        return Ok(Vec::new());
    };

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&source, &target)?;
    Ok(vec![target])
}

fn reconcile_dir_candidates(target: PathBuf, candidates: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    if target.exists() {
        return Ok(Vec::new());
    }

    let Some(source) = candidates.into_iter().find(|path| path.exists()) else {
        return Ok(Vec::new());
    };

    copy_dir_recursive(&source, &target)?;
    Ok(vec![target])
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else if !dest_path.exists() {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &dest_path)?;
        }
    }

    Ok(())
}

fn xdg_config_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(dir).join(LEGACY_APP_NAME));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join(LEGACY_APP_NAME))
}

fn xdg_data_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(dir).join(LEGACY_APP_NAME));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(LEGACY_APP_NAME)
    })
}

fn macos_application_support_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(LEGACY_APP_NAME)
    })
}

fn dedupe(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn run_artifacts_create_expected_project_paths() {
        let temp = TempDir::new().unwrap();
        let artifacts = project_run_artifacts(temp.path(), "run_1").unwrap();

        assert_eq!(
            artifacts.root(),
            temp.path().join(".imp").join("runs").join("run_1")
        );
        assert!(artifacts.root().exists());
        assert_eq!(artifacts.trace_path(), artifacts.root().join("trace.jsonl"));
        assert_eq!(
            artifacts.evidence_path(),
            artifacts.root().join("evidence.md")
        );
        assert_eq!(artifacts.diff_path(), artifacts.root().join("diff.patch"));
        assert_eq!(
            artifacts.verify_log_path(),
            artifacts.root().join("verify.log")
        );
        assert_eq!(
            artifacts.policy_log_path(),
            artifacts.root().join("policy.jsonl")
        );
        assert_eq!(
            artifacts.workflow_controller_path(),
            artifacts.root().join("workflow-controller.json")
        );
        assert_eq!(
            artifacts.workflow_contract_path(),
            artifacts.root().join("workflow-contract.json")
        );
    }

    #[test]
    fn existing_run_artifacts_reopens_without_creating_missing_directory() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("runs");
        assert!(existing_run_artifacts_under(base.clone(), "run_1").is_err());

        let created = run_artifacts_under(base.clone(), "run_1").unwrap();
        let reopened = existing_run_artifacts_under(base, "run_1").unwrap();
        assert_eq!(reopened.root(), created.root());
    }

    #[test]
    fn run_artifacts_reject_path_traversal_run_ids() {
        let temp = TempDir::new().unwrap();
        assert!(project_run_artifacts(temp.path(), "../escape").is_err());
        assert!(project_run_artifacts(temp.path(), "bad/slash").is_err());
        assert!(project_run_artifacts(temp.path(), "").is_err());
    }

    #[test]
    fn run_artifacts_under_keeps_root_inside_base() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("runs");
        let artifacts = run_artifacts_under(base.clone(), "run-abc_123").unwrap();
        assert!(artifacts.root().starts_with(&base));
    }

    #[test]
    fn work_scope_uses_project_local_store_and_global_work_root() {
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("home").join(".imp");
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();

        let scope = WorkScope::with_global_root(&project, global.clone());

        assert_eq!(scope.project_root, project.canonicalize().unwrap());
        assert_eq!(
            scope.local_store_root,
            project.canonicalize().unwrap().join(".imp").join("work")
        );
        assert_eq!(scope.global_store_root, global.join("work"));
        assert_eq!(scope.active_source, WorkStoreSource::GlobalProjectScoped);
        assert_eq!(scope.writes_target, WorkStoreSource::GlobalProjectScoped);
        assert!(scope.migration_status().contains("migration input only"));
    }

    #[test]
    fn global_root_prefers_home_imp_directory() {
        let path = global_root_from_env(Some("/tmp/home".into()), None);
        assert_eq!(path, PathBuf::from("/tmp/home/.imp"));
    }

    #[test]
    fn global_root_falls_back_to_userprofile_when_home_missing() {
        let path = global_root_from_env(None, Some("C:/Users/test".into()));
        assert_eq!(path, PathBuf::from("C:/Users/test/.imp"));
    }

    #[test]
    fn project_root_uses_dot_imp_directory() {
        assert_eq!(
            project_root(Path::new("/tmp/project")),
            PathBuf::from("/tmp/project/.imp")
        );
    }

    #[test]
    fn global_session_index_lives_under_indexes() {
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", "/tmp/home");
        assert_eq!(
            global_session_index_path(),
            PathBuf::from("/tmp/home/.imp/indexes/session_index.db")
        );
        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn reconcile_file_candidates_copies_first_existing_legacy_file() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join(".imp").join("config.toml");
        let legacy = temp.path().join("legacy").join("config.toml");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, "model = \"sonnet\"\n").unwrap();

        let migrated = reconcile_file_candidates(target.clone(), vec![legacy.clone()]).unwrap();
        assert_eq!(migrated, vec![target.clone()]);
        assert_eq!(fs::read_to_string(target).unwrap(), "model = \"sonnet\"\n");
    }

    #[test]
    fn reconcile_dir_candidates_copies_directory_tree_when_target_missing() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join(".imp").join("skills");
        let legacy = temp.path().join("legacy").join("skills").join("my-skill");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("SKILL.md"), "# Skill\n").unwrap();

        let migrated = reconcile_dir_candidates(
            target.clone(),
            vec![temp.path().join("legacy").join("skills")],
        )
        .unwrap();
        assert_eq!(migrated, vec![target.clone()]);
        assert!(target.join("my-skill").join("SKILL.md").exists());
    }
}
