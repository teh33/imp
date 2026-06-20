use std::path::{Path, PathBuf};

use serde_json::json;

use super::exec::{run_git, run_git_owned};
use super::output::{git_failure, stdout_lossy};
use super::{non_empty_param, validate_path_string, validate_ref};
use crate::error::Result;
use crate::tools::{resolve_path, ToolOutput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CurrentSecondaryWorktree {
    pub(super) main_path: PathBuf,
    pub(super) worktree_path: PathBuf,
    pub(super) branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedWorktreeEntry {
    path: String,
    branch: Option<String>,
    is_bare: bool,
    is_detached: bool,
}

pub(super) async fn current_secondary_worktree(
    cwd: &Path,
) -> Result<Option<CurrentSecondaryWorktree>> {
    let output = run_git(cwd, ["worktree", "list", "--porcelain"]).await?;
    if !output.status.success() {
        return Ok(None);
    }

    let entries = parse_worktree_list(&stdout_lossy(&output));
    let current = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let Some(current_entry) = entries
        .iter()
        .find(|entry| same_path(Path::new(&entry.path), &current))
    else {
        return Ok(None);
    };
    let Some(main_entry) = entries.first() else {
        return Ok(None);
    };
    if current_entry.path == main_entry.path {
        return Ok(None);
    }

    Ok(Some(CurrentSecondaryWorktree {
        main_path: PathBuf::from(&main_entry.path),
        worktree_path: PathBuf::from(&current_entry.path),
        branch: current_entry
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string()),
    }))
}

pub(super) async fn worktree_list_action(cwd: &Path, repo_root: &Path) -> Result<ToolOutput> {
    let output = run_git(cwd, ["worktree", "list", "--porcelain"]).await?;
    if !output.status.success() {
        return Ok(git_failure("git worktree list failed", &output));
    }

    let entries = parse_worktree_list(&stdout_lossy(&output));
    let current_secondary = current_secondary_worktree(cwd).await?;
    let mut text = String::new();
    text.push_str(&format!("repo: {}\n", repo_root.display()));
    match &current_secondary {
        Some(info) => {
            text.push_str(&format!(
                "current worktree: secondary ({}) at {}\n",
                info.branch,
                info.worktree_path.display()
            ));
            text.push_str(&format!("main worktree: {}\n", info.main_path.display()));
        }
        None => text.push_str("current worktree: main\n"),
    }
    if entries.is_empty() {
        text.push_str("registered worktrees: none\n");
    } else {
        text.push_str("registered worktrees:\n");
        for entry in &entries {
            let branch = entry.branch.as_deref().unwrap_or("(detached)");
            let mut flags = Vec::new();
            if entry.is_bare {
                flags.push("bare");
            }
            if entry.is_detached {
                flags.push("detached");
            }
            if flags.is_empty() {
                text.push_str(&format!("- {} [{}]\n", entry.path, branch));
            } else {
                text.push_str(&format!(
                    "- {} [{}] ({})\n",
                    entry.path,
                    branch,
                    flags.join(", ")
                ));
            }
        }
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "worktree_list",
            "repo_root": repo_root.display().to_string(),
            "current_secondary_worktree": current_secondary.as_ref().map(|info| json!({
                "main_path": info.main_path.display().to_string(),
                "worktree_path": info.worktree_path.display().to_string(),
                "branch": info.branch,
            })),
            "worktrees": entries.iter().map(|entry| json!({
                "path": entry.path,
                "branch": entry.branch,
                "is_bare": entry.is_bare,
                "is_detached": entry.is_detached,
            })).collect::<Vec<_>>(),
        }),
        is_error: false,
    })
}

pub(super) async fn worktree_add_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let Some(raw_worktree_path) = non_empty_param(params, "worktree_path") else {
        return Ok(ToolOutput::error(
            "Missing required parameter: worktree_path",
        ));
    };
    validate_path_string(raw_worktree_path, "worktree_path")?;
    let Some(branch) = non_empty_param(params, "branch") else {
        return Ok(ToolOutput::error("Missing required parameter: branch"));
    };
    validate_ref(branch, "branch")?;

    let start_point = non_empty_param(params, "start_point").unwrap_or("HEAD");
    validate_ref(start_point, "start_point")?;
    let worktree_path = resolve_path(cwd, raw_worktree_path);

    let output = run_git_owned(
        cwd,
        vec![
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            branch.to_string(),
            worktree_path.display().to_string(),
            start_point.to_string(),
        ],
    )
    .await?;

    if !output.status.success() {
        return Ok(git_failure("git worktree add failed", &output));
    }

    let summary = format!(
        "Created worktree {} on branch {}",
        worktree_path.display(),
        branch
    );

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: summary.clone(),
        }],
        details: json!({
            "action": "worktree_add",
            "repo_root": repo_root.display().to_string(),
            "worktree_path": worktree_path.display().to_string(),
            "branch": branch,
            "start_point": start_point,
            "recovery": {
                "undo": "git worktree_remove",
                "worktree_path": worktree_path.display().to_string(),
                "branch": branch,
                "delete_branch": true,
            },
            "summary": summary,
        }),
        is_error: false,
    })
}

pub(super) async fn worktree_remove_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let Some(raw_worktree_path) = non_empty_param(params, "worktree_path") else {
        return Ok(ToolOutput::error(
            "Missing required parameter: worktree_path",
        ));
    };
    validate_path_string(raw_worktree_path, "worktree_path")?;
    let worktree_path = resolve_path(cwd, raw_worktree_path);
    let force = params["force"].as_bool().unwrap_or(false);
    let delete_branch = params["delete_branch"].as_bool().unwrap_or(false);

    if same_path(&worktree_path, repo_root) {
        return Ok(ToolOutput::error(
            "Refusing to remove the main worktree/root checkout",
        ));
    }
    if same_path(&worktree_path, cwd) {
        return Ok(ToolOutput::error(
            "Refusing to remove the current working directory worktree",
        ));
    }

    let entries_output = run_git(cwd, ["worktree", "list", "--porcelain"]).await?;
    if !entries_output.status.success() {
        return Ok(git_failure("git worktree list failed", &entries_output));
    }
    let entries = parse_worktree_list(&stdout_lossy(&entries_output));
    let explicit_branch = non_empty_param(params, "branch");
    if let Some(branch) = explicit_branch {
        validate_ref(branch, "branch")?;
    }
    if delete_branch && explicit_branch.is_none() {
        return Ok(ToolOutput::error(
            "delete_branch=true requires explicit branch",
        ));
    }
    let matched_branch = explicit_branch.map(str::to_string).or_else(|| {
        entries
            .iter()
            .find(|entry| same_path(Path::new(&entry.path), &worktree_path))
            .and_then(|entry| entry.branch.clone())
    });

    let mut args = vec!["worktree".to_string(), "remove".to_string()];
    if force {
        args.push("--force".to_string());
    }
    args.push(worktree_path.display().to_string());

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git worktree remove failed", &output));
    }

    let mut branch_deleted = false;
    if delete_branch {
        if let Some(branch) = matched_branch.as_deref() {
            let branch_output = run_git_owned(
                cwd,
                vec![
                    "branch".to_string(),
                    if force { "-D" } else { "-d" }.to_string(),
                    branch.to_string(),
                ],
            )
            .await?;
            if !branch_output.status.success() {
                return Ok(git_failure("git branch delete failed", &branch_output));
            }
            branch_deleted = true;
        }
    }

    let summary = if branch_deleted {
        format!(
            "Removed worktree {} and deleted branch {}",
            worktree_path.display(),
            matched_branch.as_deref().unwrap_or("(unknown)")
        )
    } else {
        format!("Removed worktree {}", worktree_path.display())
    };

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: summary.clone(),
        }],
        details: json!({
            "action": "worktree_remove",
            "repo_root": repo_root.display().to_string(),
            "worktree_path": worktree_path.display().to_string(),
            "force": force,
            "delete_branch": delete_branch,
            "branch": matched_branch,
            "branch_deleted": branch_deleted,
            "recovery": {
                "guidance": "Recreate removed worktree with git worktree_add if needed; deleted branches may be recoverable from reflog.",
                "worktree_path": worktree_path.display().to_string(),
                "branch_deleted": branch_deleted,
            },
            "summary": summary,
        }),
        is_error: false,
    })
}

fn parse_worktree_list(output: &str) -> Vec<ParsedWorktreeEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;
    let mut is_detached = false;

    let push_current = |entries: &mut Vec<ParsedWorktreeEntry>,
                        current_path: &mut Option<String>,
                        current_branch: &mut Option<String>,
                        is_bare: &mut bool,
                        is_detached: &mut bool| {
        if let Some(path) = current_path.take() {
            entries.push(ParsedWorktreeEntry {
                path,
                branch: current_branch.take(),
                is_bare: *is_bare,
                is_detached: *is_detached,
            });
        }
        *is_bare = false;
        *is_detached = false;
    };

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            push_current(
                &mut entries,
                &mut current_path,
                &mut current_branch,
                &mut is_bare,
                &mut is_detached,
            );
            current_path = Some(path.to_string());
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }

    push_current(
        &mut entries,
        &mut current_path,
        &mut current_branch,
        &mut is_bare,
        &mut is_detached,
    );
    entries
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
