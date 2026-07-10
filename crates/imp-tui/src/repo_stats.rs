use ignore::{DirEntry, WalkBuilder, WalkState};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStats {
    pub primary_language: String,
    pub code_lines: u64,
    pub files: u64,
    pub symbols: Option<usize>,
    pub tests: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    files: u64,
    lines: u64,
}

pub fn scan_repo(root: &Path) -> io::Result<Option<RepoStats>> {
    let totals = count_project(root)?;
    let total_lines: u64 = totals.values().map(|stats| stats.lines).sum();
    if total_lines == 0 {
        return Ok(None);
    }

    let mut languages: Vec<_> = totals.into_iter().collect();
    languages.sort_by_key(|(_, stats)| std::cmp::Reverse(stats.lines));
    let Some((primary_language, _)) = languages.first() else {
        return Ok(None);
    };

    Ok(Some(RepoStats {
        primary_language: (*primary_language).to_string(),
        code_lines: total_lines,
        files: languages.iter().map(|(_, stats)| stats.files).sum(),
        symbols: None,
        tests: None,
    }))
}

fn count_project(root: &Path) -> io::Result<HashMap<&'static str, Stats>> {
    let totals = Arc::new(Mutex::new(HashMap::<&'static str, Stats>::new()));
    WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .ignore(true)
        .filter_entry(should_scan_entry)
        .build_parallel()
        .run(|| {
            let totals = Arc::clone(&totals);
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }
                let Some(language) = language_for(entry.path()) else {
                    return WalkState::Continue;
                };
                let Ok(lines) = count_nonblank_lines(entry.path()) else {
                    return WalkState::Continue;
                };
                let mut totals = totals.lock().expect("language totals mutex poisoned");
                let stats = totals.entry(language).or_default();
                stats.files += 1;
                stats.lines += lines;
                WalkState::Continue
            })
        });
    Ok(Arc::try_unwrap(totals)
        .expect("walk workers finished")
        .into_inner()
        .expect("language totals mutex poisoned"))
}

fn should_scan_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return true;
    }
    entry.file_name() != ".git" && !entry.path().join(".git").exists()
}

fn language_for(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    language_for_extension(extension).or_else(|| {
        let lowercase = extension.to_ascii_lowercase();
        language_for_extension(&lowercase)
    })
}

fn language_for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "rs" => "Rust",
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" | "pyw" => "Python",
        "go" => "Go",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "C++",
        "cs" => "C#",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "rb" => "Ruby",
        "php" => "PHP",
        "zig" => "Zig",
        "odin" => "Odin",
        "ex" | "exs" => "Elixir",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" | "less" => "CSS",
        "svelte" => "Svelte",
        "vue" => "Vue",
        "dart" => "Dart",
        "lua" => "Lua",
        "r" => "R",
        _ => return None,
    })
}

fn count_nonblank_lines(path: &Path) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let mut buffer = [0; 64 * 1024];
    let mut lines = 0;
    let mut has_text = false;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            match byte {
                b'\n' => {
                    lines += u64::from(has_text);
                    has_text = false;
                }
                b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {}
                _ => has_text = true,
            }
        }
    }
    Ok(lines + u64::from(has_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn recognizes_language_by_extension() {
        assert_eq!(language_for(Path::new("src/main.rs")), Some("Rust"));
        assert_eq!(language_for(Path::new("app.tsx")), Some("TypeScript"));
        assert_eq!(language_for(Path::new("README.md")), None);
    }

    #[test]
    fn recognizes_uppercase_extensions() {
        assert_eq!(language_for(Path::new("BUILD.RS")), Some("Rust"));
    }

    #[test]
    fn excludes_nested_git_repositories_and_worktrees() {
        let temp = tempfile::tempdir().expect("create temp directory");
        fs::create_dir_all(temp.path().join("src")).expect("create root source directory");
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write root source");

        let nested_repo = temp.path().join("vendor/repo");
        fs::create_dir_all(nested_repo.join(".git")).expect("create nested repository");
        fs::write(nested_repo.join("lib.rs"), "fn nested_repo() {}\n")
            .expect("write nested repository source");

        let nested_worktree = temp.path().join("worktrees/worker");
        fs::create_dir_all(&nested_worktree).expect("create nested worktree");
        fs::write(nested_worktree.join(".git"), "gitdir: elsewhere\n")
            .expect("write worktree marker");
        fs::write(nested_worktree.join("lib.rs"), "fn nested_worktree() {}\n")
            .expect("write nested worktree source");

        let stats = scan_repo(temp.path())
            .expect("scan repository")
            .expect("find source files");
        assert_eq!(stats.primary_language, "Rust");
        assert_eq!(stats.code_lines, 1);
        assert_eq!(stats.files, 1);
    }

    #[test]
    fn counts_nonblank_lines() {
        let path = std::env::temp_dir().join(format!(
            "imp-repo-stats-test-{}.txt",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time is before unix epoch")
                .as_nanos()
        ));
        fs::write(&path, "one\n\n  \t\ntwo").expect("write test file");
        assert_eq!(count_nonblank_lines(&path).expect("count lines"), 2);
        fs::remove_file(path).expect("remove test file");
    }
}
