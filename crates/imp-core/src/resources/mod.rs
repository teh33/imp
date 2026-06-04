use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::storage;

/// Discovered AGENTS.md content.
#[derive(Debug, Clone)]
pub struct AgentsMd {
    pub path: PathBuf,
    pub content: String,
}

/// Discovered skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Discovered prompt template.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
}

impl PromptTemplate {
    /// Expand `{{variable}}` placeholders with the given values.
    pub fn expand(&self, vars: &HashMap<String, String>) -> String {
        let mut result = self.content.clone();
        for (key, value) in vars {
            let placeholder = format!("{{{{{}}}}}", key);
            result = result.replace(&placeholder, value);
        }
        result
    }
}

/// Discovered soul document.
#[derive(Debug, Clone)]
pub struct SoulDoc {
    pub path: PathBuf,
    pub content: String,
}

/// Discover the nearest project soul document by walking up from cwd.
pub fn discover_project_soul(cwd: &Path) -> Option<SoulDoc> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let path = storage::project_soul_path(d);
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(SoulDoc { path, content });
        }
        dir = d.parent();
    }
    None
}

/// Suggest where a new project soul should be created.
///
/// Prefers the nearest ancestor that looks like a project root. Falls back to `cwd/.imp/soul.md`.
pub fn suggested_project_soul_path(cwd: &Path) -> PathBuf {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let looks_like_project_root = d.join(".imp").exists()
            || d.join(".git").exists()
            || d.join("Cargo.toml").exists()
            || d.join("package.json").exists()
            || d.join("pyproject.toml").exists()
            || d.join("go.mod").exists()
            || d.join("AGENTS.md").exists()
            || d.join("CLAUDE.md").exists();
        if looks_like_project_root {
            return storage::project_soul_path(d);
        }
        dir = d.parent();
    }

    cwd.join(".imp").join("soul.md")
}

/// Discover the active soul document.
///
/// Precedence:
/// 1. nearest project `.imp/soul.md` while walking up from cwd
/// 2. global `<user_config_dir>/soul.md`
pub fn discover_soul(cwd: &Path, user_config_dir: &Path) -> Option<SoulDoc> {
    if let Some(project) = discover_project_soul(cwd) {
        return Some(project);
    }

    let global = user_config_dir.join("soul.md");
    std::fs::read_to_string(&global)
        .ok()
        .map(|content| SoulDoc {
            path: global,
            content,
        })
}

fn global_agents_candidates(user_config_dir: &Path) -> [PathBuf; 3] {
    [
        user_config_dir.join("agents.md"),
        user_config_dir.join("AGENTS.md"),
        user_config_dir.join("CLAUDE.md"),
    ]
}

fn project_agents_candidates(project_dir: &Path) -> [PathBuf; 3] {
    [
        storage::project_agents_path(project_dir),
        project_dir.join("AGENTS.md"),
        project_dir.join("CLAUDE.md"),
    ]
}

fn push_agents_md_if_unique(
    results: &mut Vec<AgentsMd>,
    seen_paths: &mut HashSet<PathBuf>,
    seen_content: &mut HashSet<String>,
    path: PathBuf,
) {
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };

    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    if !seen_paths.insert(canonical_path) {
        return;
    }

    if !seen_content.insert(content.clone()) {
        return;
    }

    results.push(AgentsMd { path, content });
}

/// Discover instruction documents by walking up from cwd.
///
/// Canonical imp-native files are `.imp/agents.md` at global and project scope.
/// Legacy compatibility files (`AGENTS.md`, `CLAUDE.md`) are still read after the
/// canonical file at each scope level.
pub fn discover_agents_md(cwd: &Path, user_config_dir: &Path) -> Vec<AgentsMd> {
    let mut results = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut seen_content = HashSet::new();

    for path in global_agents_candidates(user_config_dir) {
        push_agents_md_if_unique(&mut results, &mut seen_paths, &mut seen_content, path);
    }

    let mut dir = Some(cwd);
    while let Some(d) = dir {
        for path in project_agents_candidates(d) {
            push_agents_md_if_unique(&mut results, &mut seen_paths, &mut seen_content, path);
        }
        dir = d.parent();
    }

    results
}

/// Discover skills from user and project directories.
pub fn discover_skills(cwd: &Path, user_config_dir: &Path) -> Vec<Skill> {
    let mut by_name = HashMap::new();
    let mut dirs = vec![user_config_dir.join("skills")];

    let mut ancestry = Vec::new();
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        ancestry.push(storage::project_skills_dir(current));
        dir = current.parent();
    }
    ancestry.reverse();
    dirs.extend(ancestry);

    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let skill_dir = entry.path();
                let skill_file = skill_dir.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(content) = std::fs::read_to_string(&skill_file) {
                        let name = skill_dir
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let description = extract_description(&content);
                        by_name.insert(
                            name.clone(),
                            Skill {
                                name,
                                description,
                                path: skill_file,
                            },
                        );
                    }
                }
            }
        }
    }

    let mut skills: Vec<Skill> = by_name.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Discover prompt templates.
pub fn discover_prompts(cwd: &Path, user_config_dir: &Path) -> Result<Vec<PromptTemplate>> {
    let mut prompts = Vec::new();

    let dirs = [
        user_config_dir.join("prompts"),
        storage::project_prompts_dir(cwd),
    ];

    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let name = path
                            .file_stem()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        prompts.push(PromptTemplate {
                            name,
                            path,
                            content,
                        });
                    }
                }
            }
        }
    }

    Ok(prompts)
}

/// Extract the first paragraph as a description from a markdown file.
pub fn extract_description(content: &str) -> String {
    content
        .lines()
        .skip_while(|l| l.starts_with('#') || l.trim().is_empty())
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

/// Return markdown content without leading YAML frontmatter.
pub fn strip_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content;
    };

    match rest.find("\n---") {
        Some(end) => rest[end + "\n---".len()..].trim_start_matches(['\n', '\r']),
        None => content,
    }
}

/// Render a skill body for explicit slash-command invocation.
pub fn render_skill_invocation(name: &str, content: &str, args: &str) -> String {
    let body = strip_frontmatter(content).trim();
    let args = args.trim();
    let body = if args.is_empty() {
        body.to_string()
    } else if body.contains("$ARGUMENTS") {
        body.replace("$ARGUMENTS", args)
    } else {
        format!("{body}\n\nARGUMENTS: {args}")
    };

    format!("Use the `{name}` skill.\n\n{body}")
}

#[cfg(test)]
mod tests;
