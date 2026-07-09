use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

use super::{resolve_path, truncate_head, Tool, ToolContext, ToolOutput};
use crate::config::AgentMode;
use crate::error::Result;

const DEFAULT_LOG_LIMIT: u32 = 10;
const DISPLAY_MAX_LINES: usize = 400;
const DISPLAY_MAX_BYTES: usize = 32 * 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

pub struct GitTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitActionClass {
    ReadOnly,
    Mutating,
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn label(&self) -> &str {
        "Git"
    }

    fn description(&self) -> &str {
        "Local git status, diff, log, stage, commit, restore."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "status",
                        "diff",
                        "log",
                        "merge_base",
                        "stage",
                        "commit",
                        "restore",
                        "worktree_list"
                    ],
                    "description": "Git action"
                },
                "path": {
                    "type": "string",
                    "description": "Repo/worktree path"
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths"
                },
                "all_changes": {
                    "type": "boolean",
                    "description": "Stage all changes"
                },
                "cached": {
                    "type": "boolean",
                    "description": "Diff staged changes"
                },
                "base": {
                    "type": "string",
                    "description": "Diff base ref"
                },
                "head": {
                    "type": "string",
                    "description": "Diff head ref"
                },
                "ref1": {
                    "type": "string",
                    "description": "First ref"
                },
                "ref2": {
                    "type": "string",
                    "description": "Second ref"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Log limit"
                },
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "allow_empty": {
                    "type": "boolean",
                    "description": "Allow empty commit"
                },
                "preserve_index": {
                    "type": "boolean",
                    "description": "For file-targeted commits, preserve the existing index by committing via a temporary index (default true)"
                },
                "source": {
                    "type": "string",
                    "description": "Restore source ref"
                },
            },
            "required": ["action"]
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        _call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        let action = match params["action"].as_str() {
            Some(action) => action,
            None => return Ok(ToolOutput::error("Missing required parameter: action")),
        };

        let Some(class) = action_class(action) else {
            return Ok(ToolOutput::error(format!(
                "Unknown git action \"{action}\""
            )));
        };

        if matches!(class, GitActionClass::Mutating)
            && !matches!(ctx.mode, AgentMode::Full | AgentMode::Worker)
        {
            return Ok(ToolOutput::error(format!(
                "git action `{action}` is not permitted in {:?} mode; mutating git actions are limited to full/worker execution",
                ctx.mode
            )));
        }

        let cwd = match resolve_git_cwd(&ctx.cwd, params.get("path").and_then(|v| v.as_str())) {
            Ok(path) => path,
            Err(err) => return Ok(ToolOutput::error(err)),
        };

        let repo_root = match repo_root(&cwd).await {
            Ok(path) => path,
            Err(err) => return Ok(ToolOutput::error(err)),
        };

        match action {
            "status" => status_action(&cwd, &repo_root).await,
            "diff" => diff_action(&cwd, &repo_root, &params).await,
            "log" => log_action(&cwd, &repo_root, &params).await,
            "merge_base" => merge_base_action(&cwd, &repo_root, &params).await,
            "worktree_list" => worktree_list_action(&cwd, &repo_root).await,
            "stage" => stage_action(&cwd, &repo_root, &params).await,
            "commit" => commit_action(&cwd, &repo_root, &params).await,
            "restore" => restore_action(&cwd, &repo_root, &params, &ctx).await,
            _ => Ok(ToolOutput::error(format!(
                "Unsupported git action `{action}`"
            ))),
        }
    }
}

fn action_class(action: &str) -> Option<GitActionClass> {
    match action {
        "status" | "diff" | "log" | "merge_base" | "worktree_list" => {
            Some(GitActionClass::ReadOnly)
        }
        "stage" | "commit" | "restore" => Some(GitActionClass::Mutating),
        _ => None,
    }
}

fn resolve_git_cwd(session_cwd: &Path, raw: Option<&str>) -> std::result::Result<PathBuf, String> {
    let path = match raw {
        Some(raw) if !raw.trim().is_empty() => resolve_path(session_cwd, raw),
        _ => session_cwd.to_path_buf(),
    };

    if path.is_dir() {
        return Ok(path);
    }

    if path.is_file() {
        return path.parent().map(Path::to_path_buf).ok_or_else(|| {
            format!(
                "Could not determine a working directory from file path: {}",
                path.display()
            )
        });
    }

    Err(format!(
        "git path not found or not accessible: {}",
        path.display()
    ))
}

async fn repo_root(cwd: &Path) -> std::result::Result<PathBuf, String> {
    let output = run_git(cwd, ["rev-parse", "--show-toplevel"])
        .await
        .map_err(|err| format!("Failed to run git in {}: {err}", cwd.display()))?;
    if !output.status.success() {
        return Err(not_git_repo_message(cwd, &output));
    }

    let root = stdout_trimmed(&output);
    if root.is_empty() {
        return Err(format!(
            "Failed to determine git repo root from {}",
            cwd.display()
        ));
    }

    Ok(PathBuf::from(root))
}

async fn status_action(cwd: &Path, repo_root: &Path) -> Result<ToolOutput> {
    let output = run_git(cwd, ["status", "--porcelain=v1", "--branch"]).await?;
    if !output.status.success() {
        return Ok(git_failure("git status failed", &output));
    }

    let status_text = stdout_lossy(&output);
    let mut branch_summary = String::new();
    let mut entries = Vec::new();
    let mut staged = 0u32;
    let mut unstaged = 0u32;
    let mut untracked = 0u32;

    for line in status_text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            branch_summary = rest.trim().to_string();
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let index_status = line.chars().next().unwrap_or(' ');
        let worktree_status = line.chars().nth(1).unwrap_or(' ');
        let path = line[3..].trim().to_string();
        if index_status != ' ' && index_status != '?' {
            staged += 1;
        }
        if worktree_status != ' ' && worktree_status != '?' {
            unstaged += 1;
        }
        if index_status == '?' && worktree_status == '?' {
            untracked += 1;
        }
        entries.push(json!({
            "index_status": index_status.to_string(),
            "worktree_status": worktree_status.to_string(),
            "path": path,
            "raw": line,
        }));
    }

    let head = head_sha_short(cwd)
        .await
        .unwrap_or_else(|| "unknown".to_string());
    let secondary = current_secondary_worktree(cwd).await?;
    let clean = entries.is_empty();

    let mut text = String::new();
    text.push_str(&format!("repo: {}\n", repo_root.display()));
    text.push_str(&format!(
        "branch: {}\n",
        display_or_unknown(&branch_summary)
    ));
    text.push_str(&format!("head: {head}\n"));
    text.push_str(&format!(
        "state: {}\n",
        if clean { "clean" } else { "dirty" }
    ));
    if let Some(info) = &secondary {
        text.push_str(&format!("worktree: secondary ({})\n", info.branch));
        text.push_str(&format!("main worktree: {}\n", info.main_path.display()));
    } else {
        text.push_str("worktree: main\n");
    }
    if !entries.is_empty() {
        text.push_str("changes:\n");
        for entry in &entries {
            if let Some(raw) = entry.get("raw").and_then(|v| v.as_str()) {
                text.push_str(raw);
                text.push('\n');
            }
        }
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "status",
            "repo_root": repo_root.display().to_string(),
            "branch": branch_summary,
            "head": head,
            "clean": clean,
            "counts": {
                "staged": staged,
                "unstaged": unstaged,
                "untracked": untracked,
            },
            "entries": entries,
            "secondary_worktree": secondary.as_ref().map(|info| json!({
                "main_path": info.main_path.display().to_string(),
                "worktree_path": info.worktree_path.display().to_string(),
                "branch": info.branch,
            })),
        }),
        is_error: false,
    })
}

fn non_empty_param<'a>(params: &'a serde_json::Value, field_name: &str) -> Option<&'a str> {
    params
        .get(field_name)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn validate_ref(value: &str, field_name: &str) -> std::result::Result<(), crate::error::Error> {
    if value.starts_with('-') || value.chars().any(|c| c == '\0' || c.is_control()) {
        return Err(crate::error::Error::Tool(format!(
            "{field_name} must be a safe git ref"
        )));
    }
    Ok(())
}

async fn worktree_list_action(cwd: &Path, repo_root: &Path) -> Result<ToolOutput> {
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

async fn current_secondary_worktree(cwd: &Path) -> Result<Option<CurrentSecondaryWorktree>> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentSecondaryWorktree {
    main_path: PathBuf,
    worktree_path: PathBuf,
    branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedWorktreeEntry {
    path: String,
    branch: Option<String>,
    is_bare: bool,
    is_detached: bool,
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

async fn diff_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let files = parse_string_array(params, "files")?;
    let cached = params["cached"].as_bool().unwrap_or(false);
    let base = non_empty_param(params, "base");
    let head = non_empty_param(params, "head");

    let mut args = vec!["diff".to_string()];
    if let Some(base) = base {
        validate_ref(base, "base")?;
        if let Some(head) = head {
            validate_ref(head, "head")?;
        }
        let range = match head {
            Some(head) => format!("{base}..{head}"),
            None => format!("{base}..HEAD"),
        };
        args.push(range);
    } else if cached {
        args.push("--cached".to_string());
    }

    if !files.is_empty() {
        args.push("--".to_string());
        args.extend(files.iter().cloned());
    }

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git diff failed", &output));
    }

    let diff = stdout_lossy(&output);
    let (display_content, display_note, temp_file) = truncate_for_display(&diff);
    let text = if diff.trim().is_empty() {
        "No diff.".to_string()
    } else if display_note.is_empty() {
        display_content.clone()
    } else {
        format!("{display_content}\n{display_note}")
    };

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "diff",
            "repo_root": repo_root.display().to_string(),
            "cached": cached,
            "base": base,
            "head": head,
            "files": files,
            "display_content": display_content,
            "display_note": display_note,
            "temp_file": temp_file.map(|p| p.display().to_string()),
        }),
        is_error: false,
    })
}

async fn log_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let files = parse_string_array(params, "files")?;
    let limit = params["limit"]
        .as_u64()
        .unwrap_or(DEFAULT_LOG_LIMIT as u64)
        .clamp(1, 100);

    let mut args = vec![
        "log".to_string(),
        "--oneline".to_string(),
        "--decorate".to_string(),
        "-n".to_string(),
        limit.to_string(),
    ];
    if !files.is_empty() {
        args.push("--".to_string());
        args.extend(files.iter().cloned());
    }

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git log failed", &output));
    }

    let log = stdout_lossy(&output);
    let text = if log.trim().is_empty() {
        "No commits matched.".to_string()
    } else {
        log.trim_end().to_string()
    };

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "log",
            "repo_root": repo_root.display().to_string(),
            "limit": limit,
            "files": files,
        }),
        is_error: false,
    })
}

async fn merge_base_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let Some(ref1) = non_empty_param(params, "ref1") else {
        return Ok(ToolOutput::error("Missing required parameter: ref1"));
    };
    validate_ref(ref1, "ref1")?;
    let Some(ref2) = non_empty_param(params, "ref2") else {
        return Ok(ToolOutput::error("Missing required parameter: ref2"));
    };
    validate_ref(ref2, "ref2")?;

    let output = run_git_owned(
        cwd,
        vec!["merge-base".to_string(), ref1.to_string(), ref2.to_string()],
    )
    .await?;

    if !output.status.success() {
        return Ok(git_failure("git merge-base failed", &output));
    }

    let merge_base = stdout_trimmed(&output);
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: merge_base.clone(),
        }],
        details: json!({
            "action": "merge_base",
            "repo_root": repo_root.display().to_string(),
            "ref1": ref1,
            "ref2": ref2,
            "merge_base": merge_base,
        }),
        is_error: false,
    })
}

async fn stage_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let files = parse_string_array(params, "files")?;
    let all = params
        .get("all_changes")
        .or_else(|| params.get("all"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let args = if all {
        vec!["add".to_string(), "-A".to_string()]
    } else {
        if files.is_empty() {
            return Ok(ToolOutput::error(
                "stage requires either files[] or all=true",
            ));
        }
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(files.iter().cloned());
        args
    };

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git add failed", &output));
    }

    let summary = if all {
        "Staged all changes".to_string()
    } else {
        format!("Staged {} path(s)", files.len())
    };

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: summary.clone(),
        }],
        details: json!({
            "action": "stage",
            "repo_root": repo_root.display().to_string(),
            "all_changes": all,
            "files": files,
            "recovery": {
                "undo": if all { "git reset" } else { "git reset -- <files>" },
                "files": files,
                "all_changes": all,
            },
            "summary": summary,
        }),
        is_error: false,
    })
}

async fn commit_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
) -> Result<ToolOutput> {
    let Some(message) = params["message"].as_str() else {
        return Ok(ToolOutput::error("Missing required parameter: message"));
    };
    if message.trim().is_empty() {
        return Ok(ToolOutput::error("Commit message cannot be empty"));
    }

    let allow_empty = params
        .get("allow_empty")
        .or_else(|| params.get("allowEmpty"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let files = parse_string_array(params, "files")?;
    let preserve_index = params
        .get("preserve_index")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);

    if !files.is_empty() && preserve_index {
        return targeted_commit_action(cwd, repo_root, message, allow_empty, &files).await;
    }

    let mut args = vec!["commit".to_string(), "-m".to_string(), message.to_string()];
    if allow_empty {
        args.push("--allow-empty".to_string());
    }
    if !files.is_empty() {
        args.push("--only".to_string());
        args.push("--".to_string());
        args.extend(files.iter().cloned());
    }

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git commit failed", &output));
    }

    let head = head_sha_short(cwd)
        .await
        .unwrap_or_else(|| "unknown".to_string());
    let parent = head_parent_sha_short(cwd).await;
    let stdout = stdout_trimmed(&output);
    let text = if stdout.is_empty() {
        format!("Committed {head}: {message}")
    } else {
        stdout
    };

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: text.clone() }],
        details: json!({
            "action": "commit",
            "repo_root": repo_root.display().to_string(),
            "message": message,
            "allow_empty": allow_empty,
            "head": head,
            "parent": parent,
            "recovery": {
                "commit": head,
                "parent": parent,
            },
            "summary": text,
        }),
        is_error: false,
    })
}

async fn targeted_commit_action(
    cwd: &Path,
    repo_root: &Path,
    message: &str,
    allow_empty: bool,
    files: &[String],
) -> Result<ToolOutput> {
    let diff_output = run_git_owned(
        cwd,
        ["diff", "--quiet", "HEAD", "--"]
            .into_iter()
            .map(str::to_string)
            .chain(files.iter().cloned())
            .collect(),
    )
    .await?;
    if diff_output.status.success() && !allow_empty {
        return Ok(ToolOutput::error(format!(
            "No changes to commit for targeted path(s): {}",
            files.join(", ")
        )));
    }

    let index_path = std::env::temp_dir().join(format!(
        "imp-git-targeted-index-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let index = index_path.to_string_lossy().to_string();

    let read_tree = run_git_with_env(cwd, ["read-tree", "HEAD"], Some((&index, repo_root))).await?;
    if !read_tree.status.success() {
        cleanup_temp_index(&index_path);
        return Ok(git_failure("git read-tree failed", &read_tree));
    }

    let mut add_args = vec!["add".to_string(), "--".to_string()];
    add_args.extend(files.iter().cloned());
    let add = run_git_owned_with_env(cwd, add_args, Some((&index, repo_root))).await?;
    if !add.status.success() {
        cleanup_temp_index(&index_path);
        return Ok(git_failure("git add failed for targeted commit", &add));
    }

    let write_tree = run_git_with_env(cwd, ["write-tree"], Some((&index, repo_root))).await?;
    if !write_tree.status.success() {
        cleanup_temp_index(&index_path);
        return Ok(git_failure("git write-tree failed", &write_tree));
    }
    let tree = stdout_trimmed(&write_tree);

    if !allow_empty {
        let head_tree = run_git(cwd, ["rev-parse", "HEAD^{tree}"]).await?;
        if !head_tree.status.success() {
            cleanup_temp_index(&index_path);
            return Ok(git_failure("git rev-parse HEAD tree failed", &head_tree));
        }
        if stdout_trimmed(&head_tree) == tree {
            cleanup_temp_index(&index_path);
            return Ok(ToolOutput::error(format!(
                "No changes to commit for targeted path(s): {}",
                files.join(", ")
            )));
        }
    }

    let commit_tree = run_git_owned(
        cwd,
        vec![
            "commit-tree".to_string(),
            tree,
            "-p".to_string(),
            "HEAD".to_string(),
            "-m".to_string(),
            message.to_string(),
        ],
    )
    .await?;
    if !commit_tree.status.success() {
        cleanup_temp_index(&index_path);
        return Ok(git_failure("git commit-tree failed", &commit_tree));
    }
    let new_head = stdout_trimmed(&commit_tree);

    let update_ref = run_git_owned(
        cwd,
        vec![
            "update-ref".to_string(),
            "-m".to_string(),
            format!("commit: {message}"),
            "HEAD".to_string(),
            new_head.clone(),
        ],
    )
    .await?;
    cleanup_temp_index(&index_path);
    if !update_ref.status.success() {
        return Ok(git_failure("git update-ref failed", &update_ref));
    }

    let mut reset_args = vec![
        "reset".to_string(),
        "-q".to_string(),
        "HEAD".to_string(),
        "--".to_string(),
    ];
    reset_args.extend(files.iter().cloned());
    let reset_index = run_git_owned(cwd, reset_args).await?;
    if !reset_index.status.success() {
        return Ok(git_failure(
            "git reset failed after targeted commit",
            &reset_index,
        ));
    }

    let head = head_sha_short(cwd)
        .await
        .unwrap_or_else(|| "unknown".to_string());
    let parent = head_parent_sha_short(cwd).await;
    let summary = format!(
        "Committed {head}: {message}\nIncluded targeted path(s): {}\nPreserved existing index and unrelated worktree changes.",
        files.join(", ")
    );

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: summary.clone(),
        }],
        details: json!({
            "action": "commit",
            "repo_root": repo_root.display().to_string(),
            "message": message,
            "allow_empty": allow_empty,
            "files": files,
            "preserve_index": true,
            "head": head,
            "parent": parent,
            "recovery": {
                "commit": head,
                "parent": parent,
            },
            "summary": summary,
        }),
        is_error: false,
    })
}

fn cleanup_temp_index(path: &Path) {
    let _ = std::fs::remove_file(path);
    let lock = path.with_extension("lock");
    let _ = std::fs::remove_file(lock);
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

async fn restore_action(
    cwd: &Path,
    repo_root: &Path,
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let files = parse_string_array(params, "files")?;
    if files.is_empty() {
        return Ok(ToolOutput::error("restore requires files[]"));
    }

    let snapshot_paths: Vec<PathBuf> = files.iter().map(|file| resolve_path(cwd, file)).collect();
    let checkpoint = ctx.checkpoint_state.snapshot_paths(
        &snapshot_paths,
        Some(format!("git restore in {}", cwd.display())),
    )?;

    let mut args = vec!["restore".to_string()];
    if let Some(source) = non_empty_param(params, "source") {
        validate_ref(source, "source")?;
        args.push(format!("--source={source}"));
    }
    args.push("--".to_string());
    args.extend(files.iter().cloned());

    let output = run_git_owned(cwd, args).await?;
    if !output.status.success() {
        return Ok(git_failure("git restore failed", &output));
    }

    let summary = format!("Restored {} path(s)", files.len());
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text {
            text: summary.clone(),
        }],
        details: json!({
            "action": "restore",
            "repo_root": repo_root.display().to_string(),
            "files": files,
            "checkpoint_id": checkpoint.as_ref().map(|c| c.id.clone()),
            "checkpoint_label": checkpoint.as_ref().and_then(|c| c.label.clone()),
            "recovery": {
                "checkpoint_id": checkpoint.as_ref().map(|c| c.id.clone()),
                "checkpoint_label": checkpoint.as_ref().and_then(|c| c.label.clone()),
            },
            "summary": summary,
        }),
        is_error: false,
    })
}

fn parse_string_array(
    params: &serde_json::Value,
    field_name: &str,
) -> std::result::Result<Vec<String>, crate::error::Error> {
    let Some(value) = params.get(field_name) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(crate::error::Error::Tool(format!(
            "{field_name} must be an array of strings"
        )));
    };

    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            return Err(crate::error::Error::Tool(format!(
                "{field_name} must contain only non-empty strings"
            )));
        };
        if s.chars().any(|c| c == '\0' || c.is_control()) {
            return Err(crate::error::Error::Tool(format!(
                "{field_name} must contain safe path strings"
            )));
        }
        result.push(s.to_string());
    }
    Ok(result)
}

async fn head_parent_sha_short(cwd: &Path) -> Option<String> {
    let output = run_git(cwd, ["rev-parse", "--short", "HEAD^"]).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let parent = stdout_trimmed(&output);
    if parent.is_empty() {
        None
    } else {
        Some(parent)
    }
}

async fn head_sha_short(cwd: &Path) -> Option<String> {
    let output = run_git(cwd, ["rev-parse", "--short", "HEAD"]).await.ok()?;
    if !output.status.success() {
        return None;
    }
    let head = stdout_trimmed(&output);
    if head.is_empty() {
        None
    } else {
        Some(head)
    }
}

async fn run_git<I, S>(cwd: &Path, args: I) -> std::io::Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    run_git_command(command).await
}

async fn run_git_owned(cwd: &Path, args: Vec<String>) -> std::io::Result<std::process::Output> {
    run_git(cwd, args).await
}

async fn run_git_with_env<I, S>(
    cwd: &Path,
    args: I,
    temp_index: Option<(&str, &Path)>,
) -> std::io::Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some((index, work_tree)) = temp_index {
        command
            .env("GIT_INDEX_FILE", index)
            .env("GIT_WORK_TREE", work_tree);
    }
    run_git_command(command).await
}

async fn run_git_command(mut command: Command) -> std::io::Result<std::process::Output> {
    match tokio::time::timeout(GIT_COMMAND_TIMEOUT, command.output()).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "git command timed out after {}s",
                GIT_COMMAND_TIMEOUT.as_secs()
            ),
        )),
    }
}

async fn run_git_owned_with_env(
    cwd: &Path,
    args: Vec<String>,
    temp_index: Option<(&str, &Path)>,
) -> std::io::Result<std::process::Output> {
    run_git_with_env(cwd, args, temp_index).await
}

fn stdout_lossy(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace('\r', "")
}

fn stderr_lossy(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace('\r', "")
}

fn stdout_trimmed(output: &std::process::Output) -> String {
    stdout_lossy(output).trim().to_string()
}

fn stderr_trimmed(output: &std::process::Output) -> String {
    stderr_lossy(output).trim().to_string()
}

fn not_git_repo_message(cwd: &Path, output: &std::process::Output) -> String {
    let stderr = stderr_trimmed(output);
    if stderr.is_empty() {
        format!("Not inside a git repository: {}", cwd.display())
    } else {
        format!("Not inside a git repository: {}\n{}", cwd.display(), stderr)
    }
}

fn git_failure(prefix: &str, output: &std::process::Output) -> ToolOutput {
    let stdout = stdout_trimmed(output);
    let stderr = stderr_trimmed(output);
    let combined = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => prefix.to_string(),
        (false, true) => format!("{prefix}: {stdout}"),
        (true, false) => format!("{prefix}: {stderr}"),
        (false, false) => format!("{prefix}: {stdout}\n{stderr}"),
    };
    ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text: combined }],
        details: json!({
            "success": false,
            "exit_code": output.status.code(),
            "stdout": stdout,
            "stderr": stderr,
        }),
        is_error: true,
    }
}

fn display_or_unknown(s: &str) -> &str {
    if s.trim().is_empty() {
        "unknown"
    } else {
        s
    }
}

fn truncate_for_display(text: &str) -> (String, String, Option<PathBuf>) {
    let truncated = truncate_head(text, DISPLAY_MAX_LINES, DISPLAY_MAX_BYTES);
    let content = truncated.content.trim_end().to_string();
    let note = if truncated.truncated {
        let base = format!(
            "[output truncated: showing {}/{} lines, {}/{} bytes]",
            truncated.output_lines,
            truncated.total_lines,
            truncated.output_bytes,
            truncated.total_bytes,
        );
        match &truncated.temp_file {
            Some(path) => format!("{base} full output: {}", path.display()),
            None => base,
        }
    } else {
        String::new()
    };
    (content, note, truncated.temp_file)
}

#[cfg(test)]
#[path = "git/tests.rs"]
mod tests;
