use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::WorkflowUpdateEvent;
use crate::error::Result;

pub(super) fn apply_status_update(
    yaml: &mut serde_yaml::Value,
    update_path: &str,
    value: &str,
) -> Result<()> {
    let parts = update_path.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["status"] => set_mapping_string(yaml, "status", value),
        ["steps", id, "status"] => set_nested_mapping_string(yaml, &["steps", id], "status", value),
        ["checks", id, "status"] => {
            set_nested_mapping_string(yaml, &["checks", id], "status", value)
        }
        ["prototypes", id, "status"] => {
            set_nested_mapping_string(yaml, &["prototypes", id], "status", value)
        }
        ["spec", "acceptance", id, "status"] => {
            set_nested_mapping_string(yaml, &["spec", "acceptance", id], "status", value)
        }
        _ => Err(crate::error::Error::Tool(format!(
            "unsupported workflow update path `{update_path}`"
        ))),
    }
}

pub(super) fn set_nested_mapping_string(
    yaml: &mut serde_yaml::Value,
    path: &[&str],
    key: &str,
    value: &str,
) -> Result<()> {
    let mut current = yaml;
    for segment in path {
        current = mapping_get_mut(current, segment).ok_or_else(|| {
            crate::error::Error::Tool(format!("workflow path segment `{segment}` not found"))
        })?;
    }
    set_mapping_string(current, key, value)
}

pub(super) fn set_mapping_string(
    yaml: &mut serde_yaml::Value,
    key: &str,
    value: &str,
) -> Result<()> {
    let mapping = yaml.as_mapping_mut().ok_or_else(|| {
        crate::error::Error::Tool(format!("workflow path target for `{key}` is not a map"))
    })?;
    let key_value = serde_yaml::Value::String(key.to_string());
    if !mapping.contains_key(&key_value) {
        return Err(crate::error::Error::Tool(format!(
            "workflow path key `{key}` not found"
        )));
    }
    mapping.insert(key_value, serde_yaml::Value::String(value.to_string()));
    Ok(())
}

fn mapping_get_mut<'a>(
    yaml: &'a mut serde_yaml::Value,
    key: &str,
) -> Option<&'a mut serde_yaml::Value> {
    yaml.as_mapping_mut()?
        .get_mut(serde_yaml::Value::String(key.to_string()))
}

pub(super) fn open_workflow_event_file(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(Into::into)
}

pub(super) fn append_workflow_event(
    file: &mut std::fs::File,
    event: &WorkflowUpdateEvent,
) -> Result<()> {
    serde_json::to_writer(&mut *file, event).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize event: {error}"))
    })?;
    writeln!(file)?;
    Ok(())
}
