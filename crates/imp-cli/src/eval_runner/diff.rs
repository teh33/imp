use std::fs;
use std::path::{Path, PathBuf};

use super::process::run_capture;
use super::result::EvalDiffSummary;

pub(super) async fn capture_diff(
    checkout: &Path,
    output_dir: &Path,
    commit: &str,
) -> Result<(Vec<u8>, EvalDiffSummary), Box<dyn std::error::Error>> {
    let index_output = run_capture("git", &["rev-parse", "--git-path", "index"], checkout).await?;
    let index_text = String::from_utf8(index_output.stdout)?;
    let index_path = resolve_index_path(checkout, index_text.trim());
    let temporary_index = output_dir.canonicalize()?.join("eval-index");
    if index_path.is_file() {
        fs::copy(&index_path, &temporary_index)?;
    } else {
        fs::write(&temporary_index, [])?;
    }

    let capture = capture_with_index(checkout, &temporary_index, commit).await;
    fs::remove_file(&temporary_index).ok();
    let (patch, names, stat) = capture?;
    let paths = String::from_utf8_lossy(&names)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let (insertions, deletions) = parse_numstat(&String::from_utf8_lossy(&stat));
    Ok((
        patch,
        EvalDiffSummary {
            files_changed: paths.len() as u32,
            insertions,
            deletions,
            paths,
        },
    ))
}

fn resolve_index_path(checkout: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        checkout.join(path)
    }
}

async fn capture_with_index(
    checkout: &Path,
    index: &Path,
    commit: &str,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), Box<dyn std::error::Error>> {
    run_git_with_index(checkout, index, &["add", "-A", "--", "."]).await?;
    let patch = run_git_capture_with_index(
        checkout,
        index,
        &["diff", "--cached", "--binary", commit, "--", "."],
    )
    .await?;
    let names = run_git_capture_with_index(
        checkout,
        index,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACMR",
            commit,
            "--",
        ],
    )
    .await?;
    let stat = run_git_capture_with_index(
        checkout,
        index,
        &["diff", "--cached", "--numstat", commit, "--"],
    )
    .await?;
    Ok((patch.stdout, names.stdout, stat.stdout))
}

async fn run_git_with_index(
    checkout: &Path,
    index: &Path,
    args: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    run_git_capture_with_index(checkout, index, args)
        .await
        .map(|_| ())
}

async fn run_git_capture_with_index(
    checkout: &Path,
    index: &Path,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(checkout)
        .env("GIT_INDEX_FILE", index)
        .output()
        .await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}

fn parse_numstat(output: &str) -> (u32, u32) {
    output.lines().fold((0, 0), |(added, removed), line| {
        let mut fields = line.split('\t');
        let line_added = fields
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let line_removed = fields
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        (added + line_added, removed + line_removed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_ignores_binary_markers() {
        assert_eq!(parse_numstat("10\t2\ta.rs\n-\t-\timage.png\n"), (10, 2));
    }

    #[tokio::test]
    async fn capture_includes_untracked_files_without_mutating_real_index() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.email", "eval@example.test"]);
        git(repo.path(), &["config", "user.name", "Eval Test"]);
        fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "-qm", "fixture"]);
        let head = run_capture("git", &["rev-parse", "HEAD"], repo.path())
            .await
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap();
        fs::write(repo.path().join("untracked.txt"), "new\n").unwrap();
        let output = tempfile::tempdir().unwrap();

        let (patch, summary) = capture_diff(repo.path(), output.path(), head.trim())
            .await
            .unwrap();

        assert_eq!(summary.paths, vec!["untracked.txt"]);
        assert!(String::from_utf8(patch).unwrap().contains("untracked.txt"));
        let staged = run_capture("git", &["diff", "--cached", "--name-only"], repo.path())
            .await
            .unwrap();
        assert!(staged.stdout.is_empty());
    }

    fn git(cwd: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap()
            .success());
    }
}
