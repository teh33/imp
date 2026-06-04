use super::*;
use std::fs;
use tempfile::TempDir;

// -- soul discovery --

#[test]
fn resource_discover_soul_uses_global_fallback() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    fs::write(user_dir.join("soul.md"), "# Soul\n\nglobal soul").unwrap();

    let soul = discover_soul(&cwd, &user_dir).expect("global soul should load");
    assert!(soul.content.contains("global soul"));
    assert_eq!(soul.path, user_dir.join("soul.md"));
}

#[test]
fn resource_discover_soul_prefers_nearest_project_override() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let project = dir.path().join("project");
    let nested = project.join("src").join("deep");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(project.join(".imp")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(user_dir.join("soul.md"), "# Soul\n\nglobal soul").unwrap();
    fs::write(
        project.join(".imp").join("soul.md"),
        "# Soul\n\nproject soul",
    )
    .unwrap();

    let soul = discover_soul(&nested, &user_dir).expect("project soul should load");
    assert!(soul.content.contains("project soul"));
    assert_eq!(soul.path, project.join(".imp").join("soul.md"));
}

#[test]
fn resource_discover_project_soul_walks_up_from_cwd() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    let nested = project.join("src").join("deep");
    fs::create_dir_all(project.join(".imp")).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        project.join(".imp").join("soul.md"),
        "# Soul\n\nproject soul",
    )
    .unwrap();

    let soul = discover_project_soul(&nested).expect("project soul should load");
    assert!(soul.content.contains("project soul"));
    assert_eq!(soul.path, project.join(".imp").join("soul.md"));
}

#[test]
fn resource_suggested_project_soul_path_prefers_nearest_projectish_ancestor() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    let nested = project.join("src").join("deep");
    fs::create_dir_all(&nested).unwrap();
    fs::write(project.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();

    let path = suggested_project_soul_path(&nested);
    assert_eq!(path, project.join(".imp").join("soul.md"));
}

#[test]
fn resource_discover_soul_empty_when_absent() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    assert!(discover_soul(&cwd, &user_dir).is_none());
}

// -- AGENTS.md discovery --

#[test]
fn resource_discover_agents_md_from_user_config() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("AGENTS.md"), "# Global rules").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let results = discover_agents_md(&cwd, &user_dir);
    assert!(results.iter().any(|a| a.content.contains("Global rules")));
}

#[test]
fn resource_discover_agents_md_walks_up_from_cwd() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();

    // Create AGENTS.md at the project root
    let project = dir.path().join("project");
    let subdir = project.join("src").join("deep");
    fs::create_dir_all(&subdir).unwrap();
    fs::write(project.join("AGENTS.md"), "# Project rules").unwrap();

    let results = discover_agents_md(&subdir, &user_dir);
    assert!(results.iter().any(|a| a.content.contains("Project rules")));
}

#[test]
fn resource_discover_agents_md_finds_claude_md() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("CLAUDE.md"), "# Claude config").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let results = discover_agents_md(&cwd, &user_dir);
    assert!(results.iter().any(|a| a.content.contains("Claude config")));
}

#[test]
fn resource_discover_agents_md_global_first() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let project = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(user_dir.join("AGENTS.md"), "global").unwrap();
    fs::write(project.join("AGENTS.md"), "project").unwrap();

    let results = discover_agents_md(&project, &user_dir);
    // Global should appear before project
    let global_idx = results.iter().position(|a| a.content == "global").unwrap();
    let project_idx = results.iter().position(|a| a.content == "project").unwrap();
    assert!(global_idx < project_idx);
}

#[test]
fn resource_discover_agents_md_reads_global_imp_agents_file() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("agents.md"), "global-imp").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let results = discover_agents_md(&cwd, &user_dir);
    assert!(results.iter().any(|a| a.content == "global-imp"));
}

#[test]
fn resource_discover_agents_md_prefers_project_imp_agents_file() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let project = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(project.join(".imp")).unwrap();
    fs::write(project.join(".imp").join("agents.md"), "project-imp").unwrap();
    fs::write(project.join("AGENTS.md"), "project-legacy").unwrap();

    let results = discover_agents_md(&project, &user_dir);
    let canonical_idx = results
        .iter()
        .position(|a| a.content == "project-imp")
        .unwrap();
    let legacy_idx = results
        .iter()
        .position(|a| a.content == "project-legacy")
        .unwrap();
    assert!(canonical_idx < legacy_idx);
}

#[test]
fn resource_discover_agents_md_dedupes_legacy_global_copy() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("agents.md"), "same global rules").unwrap();
    fs::write(user_dir.join("AGENTS.md"), "same global rules").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let results = discover_agents_md(&cwd, &user_dir);
    assert_eq!(
        results
            .iter()
            .filter(|a| a.content == "same global rules")
            .count(),
        1
    );
}

#[test]
fn resource_discover_agents_md_dedupes_global_when_home_is_ancestor() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join(".imp");
    let project = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(user_dir.join("agents.md"), "global rules").unwrap();

    let results = discover_agents_md(&project, &user_dir);
    assert_eq!(
        results
            .iter()
            .filter(|a| a.content == "global rules")
            .count(),
        1
    );
}

#[test]
fn resource_discover_agents_md_keeps_distinct_global_and_project_rules() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let project = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(user_dir.join("agents.md"), "global rules").unwrap();
    fs::write(project.join("AGENTS.md"), "project rules").unwrap();

    let results = discover_agents_md(&project, &user_dir);
    assert!(results.iter().any(|a| a.content == "global rules"));
    assert!(results.iter().any(|a| a.content == "project rules"));
}

#[test]
fn resource_discover_agents_md_empty_when_no_files() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    let results = discover_agents_md(&cwd, &user_dir);
    assert!(results.is_empty());
}

// -- Skills discovery --

#[test]
fn resource_discover_skills_from_user_dir() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let skills_dir = user_dir.join("skills").join("my-skill");
    fs::create_dir_all(&skills_dir).unwrap();
    fs::write(
        skills_dir.join("SKILL.md"),
        "# My Skill\n\nDoes useful things for you.\n",
    )
    .unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let skills = discover_skills(&cwd, &user_dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "my-skill");
    assert!(skills[0].description.contains("useful things"));
}

#[test]
fn resource_discover_skills_from_project_dir() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();

    let cwd = dir.path().join("project");
    let skills_dir = cwd.join(".imp").join("skills").join("project-skill");
    fs::create_dir_all(&skills_dir).unwrap();
    fs::write(
        skills_dir.join("SKILL.md"),
        "# Project Skill\n\nProject-specific automation.\n",
    )
    .unwrap();

    let skills = discover_skills(&cwd, &user_dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "project-skill");
}

#[test]
fn resource_discover_skills_from_both_dirs() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let user_skills = user_dir.join("skills").join("global-skill");
    fs::create_dir_all(&user_skills).unwrap();
    fs::write(user_skills.join("SKILL.md"), "# Global\n\nGlobal skill.\n").unwrap();

    let cwd = dir.path().join("project");
    let project_skills = cwd.join(".imp").join("skills").join("local-skill");
    fs::create_dir_all(&project_skills).unwrap();
    fs::write(project_skills.join("SKILL.md"), "# Local\n\nLocal skill.\n").unwrap();

    let skills = discover_skills(&cwd, &user_dir);
    assert_eq!(skills.len(), 2);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"global-skill"));
    assert!(names.contains(&"local-skill"));
}

#[test]
fn resource_discover_skills_walks_up_from_cwd() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();

    let project = dir.path().join("project");
    let nested = project.join("src").join("deep");
    let skills_dir = project.join(".imp").join("skills").join("project-skill");
    fs::create_dir_all(&skills_dir).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        skills_dir.join("SKILL.md"),
        "# Project Skill\n\nProject-specific automation.\n",
    )
    .unwrap();

    let skills = discover_skills(&nested, &user_dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "project-skill");
}

#[test]
fn resource_discover_skills_project_overrides_user_by_name() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let user_skill = user_dir.join("skills").join("workflow");
    fs::create_dir_all(&user_skill).unwrap();
    fs::write(user_skill.join("SKILL.md"), "# Workflow\n\nUser version.\n").unwrap();

    let project = dir.path().join("project");
    let project_skill = project.join(".imp").join("skills").join("workflow");
    fs::create_dir_all(&project_skill).unwrap();
    fs::write(
        project_skill.join("SKILL.md"),
        "# Workflow\n\nProject version.\n",
    )
    .unwrap();

    let skills = discover_skills(&project, &user_dir);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "workflow");
    assert!(skills[0].description.contains("Project version"));
    assert_eq!(skills[0].path, project_skill.join("SKILL.md"));
}

#[test]
fn resource_discover_skills_skips_dirs_without_skill_md() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let skills_dir = user_dir.join("skills").join("incomplete-skill");
    fs::create_dir_all(&skills_dir).unwrap();
    // No SKILL.md — just a random file
    fs::write(skills_dir.join("README.md"), "not a skill").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let skills = discover_skills(&cwd, &user_dir);
    assert!(skills.is_empty());
}

#[test]
fn resource_discover_skills_empty_when_no_dirs() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    let skills = discover_skills(&cwd, &user_dir);
    assert!(skills.is_empty());
}

// -- Prompt template discovery --

#[test]
fn resource_discover_prompts_from_user_dir() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let prompts_dir = user_dir.join("prompts");
    fs::create_dir_all(&prompts_dir).unwrap();
    fs::write(prompts_dir.join("review.md"), "Review this code: {{code}}").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let prompts = discover_prompts(&cwd, &user_dir).unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "review");
    assert!(prompts[0].content.contains("{{code}}"));
}

#[test]
fn resource_discover_prompts_from_project_dir() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    fs::create_dir_all(&user_dir).unwrap();

    let cwd = dir.path().join("project");
    let prompts_dir = cwd.join(".imp").join("prompts");
    fs::create_dir_all(&prompts_dir).unwrap();
    fs::write(
        prompts_dir.join("deploy.md"),
        "Deploy {{service}} to {{env}}",
    )
    .unwrap();

    let prompts = discover_prompts(&cwd, &user_dir).unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "deploy");
}

#[test]
fn resource_discover_prompts_ignores_non_md_files() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let prompts_dir = user_dir.join("prompts");
    fs::create_dir_all(&prompts_dir).unwrap();
    fs::write(prompts_dir.join("valid.md"), "prompt content").unwrap();
    fs::write(prompts_dir.join("ignored.txt"), "not a prompt").unwrap();
    fs::write(prompts_dir.join("also_ignored.toml"), "nope").unwrap();

    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();

    let prompts = discover_prompts(&cwd, &user_dir).unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "valid");
}

#[test]
fn resource_discover_prompts_empty_when_no_dirs() {
    let dir = TempDir::new().unwrap();
    let user_dir = dir.path().join("config");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&user_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();

    let prompts = discover_prompts(&cwd, &user_dir).unwrap();
    assert!(prompts.is_empty());
}

// -- Template expansion --

#[test]
fn resource_prompt_template_expand_variables() {
    let template = PromptTemplate {
        name: "test".into(),
        path: PathBuf::from("test.md"),
        content: "Hello {{name}}, welcome to {{project}}!".into(),
    };

    let mut vars = HashMap::new();
    vars.insert("name".into(), "Alice".into());
    vars.insert("project".into(), "imp".into());

    let result = template.expand(&vars);
    assert_eq!(result, "Hello Alice, welcome to imp!");
}

#[test]
fn resource_prompt_template_expand_missing_variable_left_as_is() {
    let template = PromptTemplate {
        name: "test".into(),
        path: PathBuf::from("test.md"),
        content: "Hello {{name}}, your role is {{role}}.".into(),
    };

    let mut vars = HashMap::new();
    vars.insert("name".into(), "Bob".into());
    // "role" not provided

    let result = template.expand(&vars);
    assert_eq!(result, "Hello Bob, your role is {{role}}.");
}

#[test]
fn resource_prompt_template_expand_empty_vars() {
    let template = PromptTemplate {
        name: "test".into(),
        path: PathBuf::from("test.md"),
        content: "No variables here.".into(),
    };

    let vars = HashMap::new();
    let result = template.expand(&vars);
    assert_eq!(result, "No variables here.");
}

#[test]
fn resource_prompt_template_expand_repeated_variable() {
    let template = PromptTemplate {
        name: "test".into(),
        path: PathBuf::from("test.md"),
        content: "{{x}} and {{x}} again".into(),
    };

    let mut vars = HashMap::new();
    vars.insert("x".into(), "hello".into());

    let result = template.expand(&vars);
    assert_eq!(result, "hello and hello again");
}

// -- extract_description --

#[test]
fn resource_extract_description_skips_headings() {
    let content = "# Title\n\nThis is the description.\nMore text here.\n\n## Section";
    let desc = extract_description(content);
    assert_eq!(desc, "This is the description. More text here.");
}

#[test]
fn resource_extract_description_empty_content() {
    assert_eq!(extract_description(""), "");
}

#[test]
fn resource_extract_description_truncates_at_200_chars() {
    let long_line = "A".repeat(250);
    let content = format!("# Title\n\n{}", long_line);
    let desc = extract_description(&content);
    assert_eq!(desc.len(), 200);
}
