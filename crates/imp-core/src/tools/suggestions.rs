use std::path::Path;

/// Compute the Levenshtein edit distance between two strings.
///
/// Uses a standard DP row-reduction approach — O(m*n) time, O(n) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Search for files with names similar to the missing `target` path.
///
/// Extracts the filename component, walks up to 4 directory levels from `cwd`,
/// and returns up to 3 candidates ranked by Levenshtein distance (closest first).
/// Only files with distance ≤ 3 from the target filename are included.
pub fn suggest_similar_files(cwd: &Path, target: &str) -> Vec<String> {
    let target_name = Path::new(target)
        .file_name()
        .and_then(|n: &std::ffi::OsStr| n.to_str())
        .unwrap_or(target);

    let mut candidates: Vec<(usize, String)> = Vec::new();

    const SKIP_DIRS: &[&str] = &[
        "target",
        "node_modules",
        ".git",
        "vendor",
        "dist",
        "build",
        "__pycache__",
        ".mypy_cache",
        ".tox",
        ".venv",
    ];

    let walker = walkdir::WalkDir::new(cwd)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    return !SKIP_DIRS.contains(&name);
                }
            }
            true
        })
        .filter_map(|e| e.ok());

    for entry in walker {
        if entry.file_type().is_file() {
            if let Some(name) = entry.file_name().to_str() {
                let dist = levenshtein(target_name, name);
                if dist <= 3 {
                    let rel = entry
                        .path()
                        .strip_prefix(cwd)
                        .unwrap_or(entry.path())
                        .display()
                        .to_string();
                    candidates.push((dist, rel));
                }
            }
        }
    }

    candidates.sort_by_key(|(d, _)| *d);
    candidates.truncate(3);
    candidates.into_iter().map(|(_, p)| p).collect()
}
