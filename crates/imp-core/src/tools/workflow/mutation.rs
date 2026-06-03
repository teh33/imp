use std::fs;
use std::path::Path;

use chrono::Utc;
use serde_json::json;

use super::checks::reconcile_workflow_statuses;
use super::files::workflow_id_root;
use super::status::{
    append_workflow_event, apply_status_update, open_workflow_event_file, set_nested_mapping_string,
};
use super::{ToolContext, ToolOutput, WorkflowUpdateEvent};
use crate::error::Result;
use crate::workflow::{load_workflow_raw, validate_workflow, ValidateOptions, WorkflowDocument};

pub(super) fn complete_step_action(
    workflows_root: &Path,
    id: Option<&str>,
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let id = id.ok_or_else(|| crate::error::Error::Tool("complete_step requires `id`".into()))?;
    let step_id = params
        .get("step")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("complete_step requires `step`".into()))?;
    let reason = params
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("complete_step requires `reason`".into()))?;

    let workflow_root = workflow_id_root(workflows_root, id)?;
    let workflow_path = workflow_root.join("workflow.yaml");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;
    ctx.check_write_path(&event_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;

    let raw = load_workflow_raw(&workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to read {}: {error}",
            workflow_path.display()
        ))
    })?;
    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to parse {}: {error}",
            workflow_path.display()
        ))
    })?;
    let doc: WorkflowDocument = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to load workflow document {}: {error}",
            workflow_path.display()
        ))
    })?;
    let step = doc
        .steps
        .get(step_id)
        .ok_or_else(|| crate::error::Error::Tool(format!("unknown workflow step `{step_id}`")))?;

    let mut event_file = open_workflow_event_file(&event_path)?;
    set_nested_mapping_string(&mut yaml, &["steps", step_id], "status", "done")?;
    append_workflow_event(
        &mut event_file,
        &WorkflowUpdateEvent {
            timestamp: Utc::now().to_rfc3339(),
            action: "complete_step".to_string(),
            path: format!("steps.{step_id}.status"),
            value: serde_json::Value::String("done".to_string()),
            reason: reason.to_string(),
        },
    )?;

    let mut completed_checks = Vec::new();
    for check_id in &step.checks {
        if doc.checks.contains_key(check_id) {
            set_nested_mapping_string(&mut yaml, &["checks", check_id], "status", "passed")?;
            append_workflow_event(
                &mut event_file,
                &WorkflowUpdateEvent {
                    timestamp: Utc::now().to_rfc3339(),
                    action: "complete_step".to_string(),
                    path: format!("checks.{check_id}.status"),
                    value: serde_json::Value::String("passed".to_string()),
                    reason: format!("step `{step_id}` completed: {reason}"),
                },
            )?;
            completed_checks.push(check_id.clone());
        }
    }

    let reconciled = reconcile_workflow_statuses(&mut yaml, &doc, &mut event_file)?;
    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize workflow: {error}"))
    })?;
    let candidate: WorkflowDocument = serde_yaml::from_str(&updated).map_err(|error| {
        crate::error::Error::Tool(format!(
            "workflow complete_step would produce invalid YAML/schema: {error}"
        ))
    })?;
    let diagnostics =
        validate_workflow(&candidate, &ValidateOptions::strict(workflow_root.clone()));
    if !diagnostics.is_empty() {
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(crate::error::Error::Tool(format!(
            "workflow complete_step failed validation: {rendered}"
        )));
    }

    let tmp_path = workflow_path.with_extension("yaml.tmp");
    ctx.check_write_path(&tmp_path).map_err(|reason| {
        crate::error::Error::Tool(format!("workflow complete_step denied: {reason}"))
    })?;
    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {} with {}: {error}",
            workflow_path.display(),
            tmp_path.display()
        ))
    })?;

    let text = format!("Completed workflow `{id}` step `{step_id}`.");
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "complete_step",
            "id": id,
            "step": step_id,
            "checks": completed_checks,
            "reconciled": reconciled,
            "reason": reason,
        }),
        is_error: false,
    })
}

pub(super) fn update_action(
    workflows_root: &Path,
    id: Option<&str>,
    params: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let id = id.ok_or_else(|| crate::error::Error::Tool("update requires `id`".into()))?;
    let update_path = params
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires `path`".into()))?;
    let value = params
        .get("value")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires string `value`".into()))?;
    let reason = params
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| crate::error::Error::Tool("update requires `reason`".into()))?;

    let workflow_root = workflow_id_root(workflows_root, id)?;
    let workflow_path = workflow_root.join("workflow.yaml");
    let raw = load_workflow_raw(&workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to read {}: {error}",
            workflow_path.display()
        ))
    })?;
    let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to parse {}: {error}",
            workflow_path.display()
        ))
    })?;

    apply_status_update(&mut yaml, update_path, value)?;

    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to serialize workflow: {error}"))
    })?;
    let candidate: WorkflowDocument = serde_yaml::from_str(&updated).map_err(|error| {
        crate::error::Error::Tool(format!(
            "workflow update would produce invalid YAML/schema: {error}"
        ))
    })?;
    let diagnostics =
        validate_workflow(&candidate, &ValidateOptions::strict(workflow_root.clone()));
    if !diagnostics.is_empty() {
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(crate::error::Error::Tool(format!(
            "workflow update failed validation: {rendered}"
        )));
    }

    let tmp_path = workflow_path.with_extension("yaml.tmp");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;
    ctx.check_write_path(&tmp_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;
    ctx.check_write_path(&event_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow update denied: {reason}")))?;

    let event = WorkflowUpdateEvent {
        timestamp: Utc::now().to_rfc3339(),
        action: "update".to_string(),
        path: update_path.to_string(),
        value: serde_json::Value::String(value.to_string()),
        reason: reason.to_string(),
    };
    let mut event_file = open_workflow_event_file(&event_path)?;

    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {} with {}: {error}",
            workflow_path.display(),
            tmp_path.display()
        ))
    })?;

    append_workflow_event(&mut event_file, &event)?;

    let text = format!("Updated workflow `{id}`: {update_path} = {value}");
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "id": id,
            "path": update_path,
            "value": value,
            "reason": reason
        }),
        is_error: false,
    })
}
