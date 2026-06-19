use std::io::{self, Write};
use std::path::PathBuf;

use imp_core::config::Config;
use imp_core::import::{
    detect_sources, import_agents_md, import_skills, AgentSource, DetectedSource, SkipReason,
};
use imp_llm::truncate_chars_with_suffix;

pub(crate) fn run(dry_run: bool, from: Option<&str>, auto_yes: bool) {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            eprintln!("Cannot determine home directory");
            std::process::exit(1);
        }
    };

    let sources = detect_sources(&home);
    let sources = filter_sources(sources, from);

    if sources.is_empty() {
        println!("No other agent configurations found.");
        println!("Checked: ~/.pi/agent/, ~/.claude/, ~/.codex/");
        return;
    }

    let (total_skills, total_agents_md) = print_sources(&sources);

    if dry_run {
        println!("Dry run — nothing was copied.");
        println!("Run without --dry-run to import.");
        return;
    }

    if total_skills == 0 && total_agents_md == 0 {
        println!("Nothing to import.");
        return;
    }

    if !auto_yes && !confirm_import(total_skills, total_agents_md) {
        println!("Cancelled.");
        return;
    }

    let imp_config = Config::user_config_dir();
    let imp_skills = imp_config.join("skills");
    import_skill_sources(&sources, &imp_skills);
    import_agents_files(&sources, &imp_config);

    println!("\nDone. Skills are in {}", imp_skills.display());
}

fn filter_sources(sources: Vec<DetectedSource>, from: Option<&str>) -> Vec<DetectedSource> {
    let Some(filter) = from else {
        return sources;
    };

    let target = match filter.to_lowercase().as_str() {
        "pi" => AgentSource::Pi,
        "claude" | "claude-code" => AgentSource::ClaudeCode,
        "codex" => AgentSource::Codex,
        other => {
            eprintln!("Unknown agent: {other}. Use: pi, claude, codex");
            std::process::exit(1);
        }
    };

    sources
        .into_iter()
        .filter(|source| source.agent == target)
        .collect()
}

fn print_sources(sources: &[DetectedSource]) -> (usize, usize) {
    println!("Found agent configurations:\n");
    let mut total_skills = 0;
    let mut total_agents_md = 0;

    for source in sources {
        println!("  {} ({})", source.agent.label(), source_path(source.agent));

        if !source.skills.is_empty() {
            println!("    {} skills:", source.skills.len());
            for skill in &source.skills {
                let desc = truncate_chars_with_suffix(&skill.description, 60, "…");
                println!("      - {} — {}", skill.name, desc);
            }
            total_skills += source.skills.len();
        }

        if !source.agents_md.is_empty() {
            for md in &source.agents_md {
                println!("    {} at {}", md.kind.label(), md.path.display());
            }
            total_agents_md += source.agents_md.len();
        }

        println!();
    }

    (total_skills, total_agents_md)
}

fn source_path(source: AgentSource) -> &'static str {
    match source {
        AgentSource::Pi => "~/.pi/agent/",
        AgentSource::ClaudeCode => "~/.claude/",
        AgentSource::Codex => "~/.codex/",
    }
}

fn confirm_import(total_skills: usize, total_agents_md: usize) -> bool {
    print!(
        "Import {} skills and {} instruction files into imp? [y/N] ",
        total_skills, total_agents_md
    );
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("y")
}

fn import_skill_sources(sources: &[DetectedSource], imp_skills: &std::path::Path) {
    for source in sources {
        if source.skills.is_empty() {
            continue;
        }

        match import_skills(&source.skills, imp_skills) {
            Ok(result) => {
                if !result.copied.is_empty() {
                    println!(
                        "  ✓ Imported {} skills from {}:",
                        result.copied.len(),
                        source.agent.label()
                    );
                    for name in &result.copied {
                        println!("      {name}");
                    }
                }
                for (name, reason) in &result.skipped {
                    match reason {
                        SkipReason::AlreadyExists => {
                            println!("    ⊘ {name} — already exists, skipped");
                        }
                        SkipReason::CopyFailed(err) => {
                            eprintln!("    ✗ {name} — copy failed: {err}");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  ✗ Failed to import skills from {}: {e}",
                    source.agent.label()
                );
            }
        }
    }
}

fn import_agents_files(sources: &[DetectedSource], imp_config: &std::path::Path) {
    let mut imported_agents = false;
    for source in sources {
        for md in &source.agents_md {
            if imported_agents {
                println!(
                    "    ⊘ {} from {} — already have AGENTS.md, skipped",
                    md.kind.label(),
                    source.agent.label()
                );
                continue;
            }
            match import_agents_md(md, imp_config) {
                Ok(Some(dest)) => {
                    println!(
                        "  ✓ Imported {} from {} → {}",
                        md.kind.label(),
                        source.agent.label(),
                        dest.display()
                    );
                    imported_agents = true;
                }
                Ok(None) => {
                    println!("    ⊘ AGENTS.md already exists in imp config, skipped");
                    imported_agents = true;
                }
                Err(e) => {
                    eprintln!(
                        "  ✗ Failed to import {} from {}: {e}",
                        md.kind.label(),
                        source.agent.label()
                    );
                }
            }
        }
    }
}
