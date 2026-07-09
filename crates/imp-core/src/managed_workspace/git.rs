use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::{ManagedWorkspaceError, ManagedWorkspaceResult};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub(super) async fn main_worktree(cwd: &Path) -> ManagedWorkspaceResult<PathBuf> {
    let entries = list_worktrees(cwd).await?;
    entries
        .first()
        .map(|entry| entry.path.clone())
        .ok_or_else(|| ManagedWorkspaceError::Git("git reported no worktrees".into()))
}

pub(super) async fn current_branch(cwd: &Path) -> ManagedWorkspaceResult<String> {
    let output = git(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await?;
    require_success(&output, "resolve current branch")?;
    Ok(stdout(&output))
}

pub(super) async fn head_commit(cwd: &Path) -> ManagedWorkspaceResult<String> {
    resolve_commit(cwd, "HEAD").await
}

pub(super) async fn resolve_commit(cwd: &Path, reference: &str) -> ManagedWorkspaceResult<String> {
    let output = git(
        cwd,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )
    .await?;
    require_success(&output, "resolve base commit")?;
    Ok(stdout(&output))
}

pub(super) async fn create(
    repo: &Path,
    path: &Path,
    branch: &str,
    base_commit: &str,
) -> ManagedWorkspaceResult<()> {
    let path = path.to_string_lossy().into_owned();
    let output = git(repo, &["worktree", "add", "-b", branch, &path, base_commit]).await?;
    require_success(&output, "create managed worktree")
}

pub(super) async fn contains_commit(
    cwd: &Path,
    ancestor: &str,
    descendant: &str,
) -> ManagedWorkspaceResult<bool> {
    let output = git(cwd, &["merge-base", "--is-ancestor", ancestor, descendant]).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(ManagedWorkspaceError::Git(format!(
            "inspect commit ancestry: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

pub(super) async fn branch_commit(
    repo: &Path,
    branch: &str,
) -> ManagedWorkspaceResult<Option<String>> {
    let reference = format!("refs/heads/{branch}^{{commit}}");
    let output = git(repo, &["rev-parse", "--verify", "--quiet", &reference]).await?;
    match output.status.code() {
        Some(0) => Ok(Some(stdout(&output))),
        Some(1) => Ok(None),
        _ => Err(ManagedWorkspaceError::Git(format!(
            "inspect managed branch: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

pub(super) async fn fast_forward(
    main_worktree: &Path,
    candidate_commit: &str,
) -> ManagedWorkspaceResult<String> {
    let output = git(main_worktree, &["merge", "--ff-only", candidate_commit]).await?;
    require_success(&output, "fast-forward integration target")?;
    head_commit(main_worktree).await
}

pub(super) async fn remove(repo: &Path, path: &Path, force: bool) -> ManagedWorkspaceResult<()> {
    let path = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    let output = git(repo, &args).await?;
    require_success(&output, "remove managed worktree")
}

pub(super) async fn delete_branch(repo: &Path, branch: &str) -> ManagedWorkspaceResult<()> {
    if branch_commit(repo, branch).await?.is_none() {
        return Ok(());
    }
    let output = git(repo, &["branch", "-D", branch]).await?;
    require_success(&output, "delete managed branch")
}

pub(super) async fn delete_branch_if_matches(
    repo: &Path,
    branch: &str,
    expected_commit: &str,
) -> ManagedWorkspaceResult<()> {
    match branch_commit(repo, branch).await? {
        None => Ok(()),
        Some(commit) if commit == expected_commit => {
            let reference = format!("refs/heads/{branch}");
            let output = git(repo, &["update-ref", "-d", &reference, expected_commit]).await?;
            require_success(&output, "delete pinned managed branch")
        }
        Some(_) => Err(ManagedWorkspaceError::Unsafe(
            "managed branch moved after its candidate commit was pinned".into(),
        )),
    }
}

pub(super) async fn status_paths(cwd: &Path) -> ManagedWorkspaceResult<Vec<PathBuf>> {
    let output = git(cwd, &["status", "--porcelain=v1", "-z"]).await?;
    require_success(&output, "inspect managed workspace")?;
    parse_status_paths(&output.stdout)
}

fn parse_status_paths(output: &[u8]) -> ManagedWorkspaceResult<Vec<PathBuf>> {
    let entries = output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    let mut index = 0;
    while index < entries.len() {
        let entry = entries[index];
        if entry.len() < 4 || entry[2] != b' ' {
            return Err(ManagedWorkspaceError::Git(
                "unexpected git status porcelain record".into(),
            ));
        }
        paths.insert(PathBuf::from(
            String::from_utf8_lossy(&entry[3..]).into_owned(),
        ));
        if matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C') {
            index += 1;
            let source = entries.get(index).ok_or_else(|| {
                ManagedWorkspaceError::Git("git status rename is missing its source path".into())
            })?;
            paths.insert(PathBuf::from(String::from_utf8_lossy(source).into_owned()));
        }
        index += 1;
    }
    Ok(paths.into_iter().collect())
}

pub(super) async fn changed_paths(
    cwd: &Path,
    base_commit: &str,
) -> ManagedWorkspaceResult<Vec<PathBuf>> {
    let range = format!("{base_commit}...HEAD");
    let output = git(cwd, &["diff", "--name-only", "-z", &range]).await?;
    require_success(&output, "inspect committed workspace changes")?;
    let mut paths: BTreeSet<PathBuf> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| PathBuf::from(String::from_utf8_lossy(entry).into_owned()))
        .collect();
    paths.extend(status_paths(cwd).await?);
    Ok(paths.into_iter().collect())
}

pub(super) async fn list_worktrees(cwd: &Path) -> ManagedWorkspaceResult<Vec<GitWorktree>> {
    let output = git(cwd, &["worktree", "list", "--porcelain"]).await?;
    require_success(&output, "list git worktrees")?;
    let mut entries = Vec::new();
    let mut path = None;
    let mut branch = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            if let Some(previous) = path.replace(PathBuf::from(value)) {
                entries.push(GitWorktree {
                    path: previous,
                    branch: branch.take(),
                });
            }
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_string());
        } else if line.is_empty() {
            if let Some(path) = path.take() {
                entries.push(GitWorktree {
                    path,
                    branch: branch.take(),
                });
            }
        }
    }
    if let Some(path) = path {
        entries.push(GitWorktree { path, branch });
    }
    Ok(entries)
}

async fn git(cwd: &Path, args: &[&str]) -> ManagedWorkspaceResult<std::process::Output> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| ManagedWorkspaceError::Git("git command timed out".into()))?
        .map_err(|error| ManagedWorkspaceError::Git(format!("failed to run git: {error}")))?;
    Ok(output)
}

fn require_success(output: &std::process::Output, action: &str) -> ManagedWorkspaceResult<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(ManagedWorkspaceError::Git(format!("{action}: {stderr}")))
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
