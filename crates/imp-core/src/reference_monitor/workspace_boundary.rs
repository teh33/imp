use std::path::{Path, PathBuf};

pub(super) fn contains(cwd: &Path, path: &Path) -> bool {
    let root = physical_path(cwd);
    let candidate = physical_path(path);
    if candidate.starts_with(&root) {
        return true;
    }

    match (git_common_dir(&root), git_common_dir(&candidate)) {
        (Some(root_git), Some(candidate_git)) => root_git == candidate_git,
        _ => false,
    }
}

fn git_common_dir(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() { path } else { path.parent()? };
    loop {
        let marker = current.join(".git");
        if marker.is_dir() {
            return marker.canonicalize().ok();
        }
        if marker.is_file() {
            return linked_common_dir(&marker);
        }
        current = current.parent()?;
    }
}

fn linked_common_dir(marker: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(marker).ok()?;
    let git_dir = contents.trim().strip_prefix("gitdir:")?.trim();
    let git_dir = resolve_from(marker.parent()?, Path::new(git_dir));
    let back_pointer = std::fs::read_to_string(git_dir.join("gitdir")).ok()?;
    let registered_marker = resolve_from(&git_dir, Path::new(back_pointer.trim()));
    if registered_marker.canonicalize().ok()? != marker.canonicalize().ok()? {
        return None;
    }
    let common_file = git_dir.join("commondir");
    if !common_file.is_file() {
        return git_dir.canonicalize().ok();
    }
    let common = std::fs::read_to_string(common_file).ok()?;
    resolve_from(&git_dir, Path::new(common.trim()))
        .canonicalize()
        .ok()
}

fn resolve_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn physical_path(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let mut suffix = Vec::new();
    let mut ancestor = path;
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(mut physical) = parent.canonicalize() {
            for name in suffix.iter().rev() {
                physical.push(name);
            }
            return physical;
        }
        ancestor = parent;
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::contains;

    #[test]
    fn registered_worktree_is_inside_repository_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktree");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-qm", "initial"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-qb",
                "feature/test",
                worktree.to_str().unwrap(),
            ],
        );

        assert!(contains(&repo, &worktree.join("new/file.rs")));
        assert!(!contains(&repo, &temp.path().join("unrelated/file.rs")));

        let unregistered = temp.path().join("unregistered");
        std::fs::create_dir(&unregistered).unwrap();
        std::fs::copy(worktree.join(".git"), unregistered.join(".git")).unwrap();
        assert!(!contains(&repo, &unregistered.join("file.rs")));
    }

    fn git(cwd: &std::path::Path, args: &[&str]) {
        assert!(Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}
