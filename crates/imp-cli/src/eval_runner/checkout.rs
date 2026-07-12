mod source_guard;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::process::{run_capture, run_git};
use super::spec::{self, EvalTaskSpec};

const OWNER_FILE: &str = "imp-eval-owner.json";

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct CheckoutOwner {
    task: String,
    source: String,
}

impl CheckoutOwner {
    fn for_spec(spec: &EvalTaskSpec) -> Self {
        Self {
            task: spec.id.clone(),
            source: spec
                .fixture
                .as_ref()
                .map(|path| format!("fixture:{}", path.display()))
                .unwrap_or_else(|| spec.repo.clone()),
        }
    }
}

pub(super) use source_guard::FixtureSourceGuard;

pub(super) async fn prepare_checkout(
    spec: &EvalTaskSpec,
    spec_path: &Path,
    checkout: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(fixture) = &spec.fixture {
        return prepare_fixture_checkout(spec, spec_path, fixture, checkout).await;
    }
    prepare_remote_checkout(spec, checkout).await?;
    Ok(spec.commit.clone())
}

async fn prepare_remote_checkout(
    spec: &EvalTaskSpec,
    checkout: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if checkout.join(".git").is_dir() {
        require_owned_checkout(spec, checkout)?;
        run_git(&["remote", "set-url", "origin", &spec.repo], checkout).await?;
        run_git(&["fetch", "origin", &spec.commit], checkout).await?;
    } else {
        clone_remote(spec, checkout).await?;
        write_checkout_owner(spec, checkout)?;
    }
    run_git(&["reset", "--hard", &spec.commit], checkout).await?;
    run_git(&["clean", "-fd"], checkout).await?;
    Ok(())
}

async fn clone_remote(
    spec: &EvalTaskSpec,
    checkout: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if checkout.exists() {
        return Err(format!(
            "eval checkout exists but is not a git repository: {}",
            checkout.display()
        )
        .into());
    }
    let parent = checkout
        .parent()
        .ok_or("eval checkout path has no parent directory")?;
    let name = checkout
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("eval checkout path has an invalid file name")?;
    let output = tokio::process::Command::new("git")
        .args(["clone", "--no-checkout", &spec.repo, name])
        .current_dir(parent)
        .output()
        .await?;
    if output.status.success() {
        run_git(&["fetch", "origin", &spec.commit], checkout).await?;
        Ok(())
    } else {
        Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into())
    }
}

async fn prepare_fixture_checkout(
    spec: &EvalTaskSpec,
    spec_path: &Path,
    fixture: &Path,
    checkout: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    if checkout.exists() {
        if !checkout.join(".git").is_dir() {
            return Err(format!(
                "eval checkout exists but is not a git repository: {}",
                checkout.display()
            )
            .into());
        }
        require_owned_checkout(spec, checkout)?;
        fs::remove_dir_all(checkout)?;
    }
    let fixture = spec::resolve_relative_to_spec(spec_path, fixture);
    copy_fixture_tree(&fixture, checkout)?;
    run_git(&["init", "-q"], checkout).await?;
    write_checkout_owner(spec, checkout)?;
    run_git(&["add", "-A"], checkout).await?;
    commit_fixture(checkout).await?;
    let head = run_capture("git", &["rev-parse", "HEAD"], checkout).await?;
    Ok(String::from_utf8(head.stdout)?.trim().to_string())
}

pub(super) async fn commit_setup_baseline(
    checkout: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    run_git(&["add", "-A"], checkout).await?;
    let staged = tokio::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(checkout)
        .status()
        .await?;
    if staged.success() {
        return current_head(checkout).await;
    }
    commit_with_identity(checkout, "eval setup").await?;
    current_head(checkout).await
}

async fn current_head(checkout: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let head = run_capture("git", &["rev-parse", "HEAD"], checkout).await?;
    Ok(String::from_utf8(head.stdout)?.trim().to_string())
}

async fn commit_with_identity(
    checkout: &Path,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = tokio::process::Command::new("git")
        .args(["commit", "-qm", message])
        .current_dir(checkout)
        .env("GIT_AUTHOR_NAME", "imp eval")
        .env("GIT_AUTHOR_EMAIL", "eval@imp.local")
        .env("GIT_COMMITTER_NAME", "imp eval")
        .env("GIT_COMMITTER_EMAIL", "eval@imp.local")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .await?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "failed to commit eval baseline: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
    .into())
}

async fn commit_fixture(checkout: &Path) -> Result<(), Box<dyn std::error::Error>> {
    commit_with_identity(checkout, "eval fixture").await
}

fn owner_path(checkout: &Path) -> std::path::PathBuf {
    checkout.join(".git").join(OWNER_FILE)
}

fn write_checkout_owner(
    spec: &EvalTaskSpec,
    checkout: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        owner_path(checkout),
        serde_json::to_vec(&CheckoutOwner::for_spec(spec))?,
    )?;
    Ok(())
}

fn require_owned_checkout(
    spec: &EvalTaskSpec,
    checkout: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let owner: CheckoutOwner =
        serde_json::from_slice(&fs::read(owner_path(checkout)).map_err(|_| {
            format!(
                "refusing to reset unowned eval checkout: {}",
                checkout.display()
            )
        })?)?;
    if owner != CheckoutOwner::for_spec(spec) {
        return Err(format!(
            "refusing to reuse eval checkout owned by another task or source: {}",
            checkout.display()
        )
        .into());
    }
    Ok(())
}

pub(super) fn configure_eval_excludes(checkout: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let exclude = checkout.join(".git/info/exclude");
    let mut content = fs::read_to_string(&exclude).unwrap_or_default();
    for pattern in [".imp/", ".venv/", "__pycache__/", "*.pyc"] {
        if !content.lines().any(|line| line.trim() == pattern) {
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(pattern);
            content.push('\n');
        }
    }
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(exclude, content)?;
    Ok(())
}

fn copy_fixture_tree(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_fixture_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(format!(
                "eval fixtures may contain only files and directories: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}
