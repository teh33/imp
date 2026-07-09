use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::super::CreateManagedWorkspace;

pub(super) fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(super) fn repo() -> TempDir {
    let repo = TempDir::new().expect("temp repo");
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["config", "user.email", "imp@example.test"]);
    git(repo.path(), &["config", "user.name", "imp test"]);
    std::fs::write(repo.path().join("README.md"), "base\n").expect("write fixture");
    git(repo.path(), &["add", "README.md"]);
    git(repo.path(), &["commit", "-qm", "base"]);
    repo
}

pub(super) fn request(id: &str) -> CreateManagedWorkspace {
    CreateManagedWorkspace {
        id: Some(id.into()),
        run_id: format!("run-{id}"),
        task: Some(format!("task {id}")),
        base_ref: None,
    }
}

pub(super) fn commit_file(worktree: &Path, path: &str, content: &str, message: &str) {
    std::fs::write(worktree.join(path), content).expect("write managed change");
    git(worktree, &["add", path]);
    git(worktree, &["commit", "-qm", message]);
}
