use crate::resources::SoulDoc;

pub const DEFAULT_IDENTITY: &str = "You are imp, a professional coding agent.";

pub fn soul_identity_text(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        return trimmed.to_string();
    }
    DEFAULT_IDENTITY.to_string()
}

pub fn default_soul_markdown() -> String {
    format!("# Soul\n\n{}\n", DEFAULT_IDENTITY)
}

pub fn write_default_soul_if_missing(path: &std::path::Path) -> crate::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, default_soul_markdown())?;
    Ok(true)
}

pub fn soul_prompt_block(soul: &SoulDoc) -> String {
    let mut s = String::from("Soul:\n");
    s.push_str(&soul.content);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_default_file_write_only_happens_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("soul.md");
        assert!(write_default_soul_if_missing(&path).unwrap());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("# Soul"));
        assert!(first.contains(DEFAULT_IDENTITY));
        assert!(!write_default_soul_if_missing(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn soul_identity_uses_first_meaningful_authored_line() {
        let content = "# Soul\n\n- note\n\nYou are Sol, a thoughtful collaborator.\n";
        assert_eq!(
            soul_identity_text(content),
            "You are Sol, a thoughtful collaborator."
        );
    }

    #[test]
    fn soul_identity_falls_back_to_default_when_empty() {
        assert_eq!(soul_identity_text("# Soul\n\n- note\n"), DEFAULT_IDENTITY);
    }
}
