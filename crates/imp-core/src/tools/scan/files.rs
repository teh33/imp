use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

pub fn collect_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(if is_supported(root) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        });
    }

    if !root.exists() {
        return Err(Error::Tool(format!(
            "scan path not found: {}",
            root.display()
        )));
    }

    if let Some(files) = git_tracked_source_files(root) {
        return Ok(files);
    }

    collect_source_files_with_ignore(root)
}

fn collect_source_files_with_ignore(root: &Path) -> Result<Vec<PathBuf>> {
    let is_git_repo = is_inside_git_worktree(root);
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .hidden(!is_git_repo)
        .follow_links(false)
        .threads(std::thread::available_parallelism().map_or(1, usize::from));

    if !is_git_repo {
        if let Some(overrides) = non_git_source_overrides(root) {
            builder.overrides(overrides);
        }
    }

    let mut files = Vec::new();
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
            && is_supported(entry.path())
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn is_inside_git_worktree(root: &Path) -> bool {
    Command::new("git")
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|stdout| stdout.trim() == "true")
}

fn non_git_source_overrides(root: &Path) -> Option<ignore::overrides::Override> {
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    for pattern in non_git_source_ignore_patterns() {
        builder.add(pattern).ok()?;
    }
    builder.build().ok()
}

fn non_git_source_ignore_patterns() -> &'static [&'static str] {
    const COMMON: &[&str] = &[
        "!**/node_modules/",
        "!**/__pycache__/",
        "!**/venv/",
        "!**/.venv/",
        "!**/vendor/",
        "!**/dist/",
        "!**/build/",
        "!**/.next/",
        "!**/coverage/",
        "!**/target/debug/",
        "!**/target/release/",
        "!**/target/rust-analyzer/",
        "!**/target/criterion/",
        #[cfg(target_os = "macos")]
        "!**/Library/Application Support/",
        #[cfg(target_os = "macos")]
        "!**/Library/Caches/",
        #[cfg(target_os = "macos")]
        "!**/Library/Group Containers/",
        #[cfg(target_os = "macos")]
        "!**/Library/Containers/",
        #[cfg(target_os = "windows")]
        "!**/bin/Debug/",
        #[cfg(target_os = "windows")]
        "!**/bin/Release/",
        #[cfg(target_os = "windows")]
        "!**/Program Files/",
        #[cfg(target_os = "windows")]
        "!**/Program Files (x86)/",
        #[cfg(target_os = "windows")]
        "!**/AppData/Local/",
        #[cfg(target_os = "windows")]
        "!**/AppData/Roaming/",
    ];
    COMMON
}

fn git_tracked_source_files(root: &Path) -> Option<Vec<PathBuf>> {
    let (dir, pathspec) = if root.is_file() {
        (
            root.parent()?,
            root.file_name()?.to_string_lossy().to_string(),
        )
    } else {
        (root, ".".to_string())
    };
    let output = Command::new("git")
        .arg("ls-files")
        .arg("-z")
        .arg("--")
        .arg(&pathspec)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .map(|relative| dir.join(relative))
        .filter(|path| is_supported(path))
        .collect::<Vec<_>>();
    Some(files)
}

pub fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "py"
                | "pyw"
                | "rs"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "ts"
                | "tsx"
                | "go"
                | "ex"
                | "exs"
                | "rb"
                | "pl"
                | "pm"
                | "t"
                | "lua"
                | "luau"
                | "zig"
                | "zon"
                | "odin"
                | "swift"
                | "kt"
                | "kts"
                | "java"
                | "c"
                | "h"
                | "cs"
                | "cc"
                | "cpp"
                | "cxx"
                | "c++"
                | "hpp"
                | "hh"
                | "hxx"
                | "h++"
                | "php"
                | "scala"
                | "sc"
                | "dart"
        )
    )
}
