use std::fs;
use std::path::{Path, PathBuf};

use super::model::SubagentMapping;
use crate::agent::SubagentRunId;
use crate::error::{Error, Result};

fn mapping_path(cwd: &Path, parent: &str, child: &str) -> Result<PathBuf> {
    valid_id(parent)?;
    valid_id(child)?;
    Ok(cwd
        .join(".imp")
        .join("runs")
        .join(parent)
        .join("subagents")
        .join(format!("{child}.json")))
}

pub(super) fn save_mapping(cwd: &Path, mapping: &SubagentMapping) -> Result<()> {
    let path = mapping_path(cwd, &mapping.parent_run_id, &mapping.child_run_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Tool("invalid subagent mapping path".into()))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(mapping)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub(super) fn load_mapping(cwd: &Path, child: &SubagentRunId) -> Result<SubagentMapping> {
    let runs = cwd.join(".imp").join("runs");
    let entries = fs::read_dir(&runs).map_err(|_| {
        Error::Tool(format!(
            "stale or unknown subagent child id `{}`",
            child.as_str()
        ))
    })?;
    let mut mappings = Vec::new();
    for entry in entries.flatten() {
        let path = entry
            .path()
            .join("subagents")
            .join(format!("{}.json", child.as_str()));
        if path.is_file() {
            mappings.push(path);
        }
    }
    if mappings.len() != 1 {
        return Err(Error::Tool(format!(
            "stale or unknown subagent child id `{}`",
            child.as_str()
        )));
    }
    serde_json::from_slice(&fs::read(&mappings[0])?)
        .map_err(|error| Error::Tool(format!("invalid subagent mapping: {error}")))
}

fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::Tool(
            "subagent child and parent ids must contain only ASCII letters, numbers, '-' or '_'"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ParentLooprRun {
    version: u32,
    loopr_run_id: String,
}

pub(super) fn load_parent_run(cwd: &Path, parent: &str) -> Result<Option<String>> {
    let path = parent_run_path(cwd, parent)?;
    match fs::read(path) {
        Ok(content) => serde_json::from_slice::<ParentLooprRun>(&content)
            .map(|mapping| Some(mapping.loopr_run_id))
            .map_err(|error| Error::Tool(format!("invalid loopr parent run mapping: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::from(error)),
    }
}

pub(super) fn save_parent_run(cwd: &Path, parent: &str, loopr_run_id: &str) -> Result<()> {
    let path = parent_run_path(cwd, parent)?;
    let parent_dir = path
        .parent()
        .ok_or_else(|| Error::Tool("invalid loopr parent run mapping path".into()))?;
    fs::create_dir_all(parent_dir)?;
    let temporary = path.with_extension("json.tmp");
    let record = ParentLooprRun {
        version: 1,
        loopr_run_id: loopr_run_id.to_string(),
    };
    fs::write(temporary.as_path(), serde_json::to_vec_pretty(&record)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn parent_run_path(cwd: &Path, parent: &str) -> Result<PathBuf> {
    valid_id(parent)?;
    Ok(cwd
        .join(".imp")
        .join("runs")
        .join(parent)
        .join("subagents")
        .join("loopr-run.json"))
}
