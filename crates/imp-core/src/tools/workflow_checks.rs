use std::fs::{self, File};
use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use super::workflow_render::CaseExt;
use super::workflow_status::{
    append_workflow_event, open_workflow_event_file, set_mapping_string, set_nested_mapping_string,
};
use super::ToolContext;
use super::WorkflowUpdateEvent;
use crate::error::Result;
use crate::workflow::{CheckKind, CheckStatus, WorkflowCheck, WorkflowDocument};

#[derive(Debug, Clone, Serialize)]
pub(super) struct WorkflowCommandCheckRun {
    pub(super) check: String,
    command: String,
    pub(super) status: String,
    pub(super) exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct WorkflowCommandStepRun {
    pub(super) step: String,
    pub(super) step_status: String,
    pub(super) checks: Vec<WorkflowCommandCheckRun>,
}

pub(super) struct CommandCheckRunSummary {
    pub(super) checks: Vec<WorkflowCommandCheckRun>,
    pub(super) step_status: String,
    pub(super) reconciled: Vec<String>,
}

pub(super) async fn run_command_checks(
    workflows_root: &Path,
    workflow_root: &Path,
    doc: &WorkflowDocument,
    step_id: &str,
    ctx: &ToolContext,
) -> Result<Option<CommandCheckRunSummary>> {
    let Some(step) = doc.steps.get(step_id) else {
        return Ok(None);
    };
    let runnable = step
        .checks
        .iter()
        .filter_map(|check_id| doc.checks.get(check_id).map(|check| (check_id, check)))
        .filter(|(_, check)| {
            matches!(
                check.kind,
                CheckKind::Command
                    | CheckKind::Presence
                    | CheckKind::Absence
                    | CheckKind::ChangedFiles
            )
        })
        .filter(|(_, check)| matches!(check.status, CheckStatus::Pending))
        .collect::<Vec<_>>();
    if runnable.is_empty() {
        return Ok(None);
    }

    let workflow_path = workflow_root.join("workflow.yaml");
    let event_path = workflow_root.join("events.jsonl");
    ctx.check_write_path(&workflow_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow run denied: {reason}")))?;
    ctx.check_write_path(&event_path)
        .map_err(|reason| crate::error::Error::Tool(format!("workflow run denied: {reason}")))?;

    let raw = fs::read_to_string(&workflow_path).map_err(|error| {
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

    let mut event_file = open_workflow_event_file(&event_path)?;
    let mut executed = Vec::new();
    let mut failed_check = false;
    let cwd = workflows_root
        .parent()
        .and_then(Path::parent)
        .unwrap_or(workflows_root);
    for (check_id, check) in runnable {
        let (status, reason, exit_code) = match evaluate_pending_check(check, cwd).await {
            Ok(outcome) => outcome,
            Err(error) => {
                failed_check = true;
                ("failed".to_string(), error, None)
            }
        };
        if status != "passed" {
            failed_check = true;
        }
        set_nested_mapping_string(&mut yaml, &["checks", check_id], "status", &status)?;
        append_workflow_event(
            &mut event_file,
            &WorkflowUpdateEvent {
                timestamp: Utc::now().to_rfc3339(),
                action: "run".to_string(),
                path: format!("checks.{check_id}.status"),
                value: serde_json::Value::String(status.clone()),
                reason,
            },
        )?;
        executed.push(WorkflowCommandCheckRun {
            check: check_id.clone(),
            command: check
                .command
                .clone()
                .unwrap_or_else(|| format!("{:?}", check.kind).to_case()),
            status,
            exit_code,
        });
    }

    let step_status = if failed_check { "failed" } else { "done" };
    set_nested_mapping_string(&mut yaml, &["steps", step_id], "status", step_status)?;
    append_workflow_event(
        &mut event_file,
        &WorkflowUpdateEvent {
            timestamp: Utc::now().to_rfc3339(),
            action: "run".to_string(),
            path: format!("steps.{step_id}.status"),
            value: serde_json::Value::String(step_status.to_string()),
            reason: format!("command checks completed with step status `{step_status}`"),
        },
    )?;

    let reconciled = reconcile_workflow_statuses(&mut yaml, doc, &mut event_file)?;

    let updated = serde_yaml::to_string(&yaml).map_err(|error| {
        crate::error::Error::Tool(format!("failed to render workflow yaml: {error}"))
    })?;
    let tmp_path = workflow_path.with_extension("yaml.tmp");
    fs::write(&tmp_path, updated).map_err(|error| {
        crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
    })?;
    fs::rename(&tmp_path, &workflow_path).map_err(|error| {
        crate::error::Error::Tool(format!(
            "failed to replace {}: {error}",
            workflow_path.display()
        ))
    })?;

    Ok(Some(CommandCheckRunSummary {
        checks: executed,
        step_status: step_status.to_string(),
        reconciled,
    }))
}

pub(super) fn reconcile_workflow_statuses(
    yaml: &mut serde_yaml::Value,
    doc: &WorkflowDocument,
    event_file: &mut File,
) -> Result<Vec<String>> {
    let mut reconciled = Vec::new();
    let check_passed = |check_id: &str, yaml: &serde_yaml::Value| -> bool {
        yaml.get("checks")
            .and_then(|checks| checks.get(check_id))
            .and_then(|check| check.get("status"))
            .and_then(|status| status.as_str())
            == Some("passed")
    };

    for (acceptance_id, criterion) in &doc.spec.acceptance {
        if !criterion.checks.is_empty()
            && criterion
                .checks
                .iter()
                .all(|check| check_passed(check, yaml))
        {
            let path = format!("spec.acceptance.{acceptance_id}.status");
            set_nested_mapping_string(
                yaml,
                &["spec", "acceptance", acceptance_id],
                "status",
                "done",
            )?;
            append_workflow_event(
                event_file,
                &WorkflowUpdateEvent {
                    timestamp: Utc::now().to_rfc3339(),
                    action: "reconcile".to_string(),
                    path: path.clone(),
                    value: serde_json::Value::String("done".to_string()),
                    reason: "all acceptance checks passed".to_string(),
                },
            )?;
            reconciled.push(path);
        }
    }

    let closeout_ready = doc
        .closeout
        .done
        .requires
        .iter()
        .all(|check| check_passed(check, yaml));
    let all_steps_terminal = yaml
        .get("steps")
        .and_then(|steps| steps.as_mapping())
        .map(|steps| {
            steps.values().all(|step| {
                matches!(
                    step.get("status").and_then(|status| status.as_str()),
                    Some("done" | "done_with_concerns" | "failed" | "blocked" | "skipped")
                )
            })
        })
        .unwrap_or(false);
    if closeout_ready && all_steps_terminal {
        set_mapping_string(yaml, "status", "done")?;
        append_workflow_event(
            event_file,
            &WorkflowUpdateEvent {
                timestamp: Utc::now().to_rfc3339(),
                action: "reconcile".to_string(),
                path: "status".to_string(),
                value: serde_json::Value::String("done".to_string()),
                reason: "closeout requirements passed and all steps are terminal".to_string(),
            },
        )?;
        reconciled.push("status".to_string());
    }

    Ok(reconciled)
}

async fn evaluate_pending_check(
    check: &WorkflowCheck,
    cwd: &Path,
) -> std::result::Result<(String, String, Option<i32>), String> {
    match check.kind {
        CheckKind::Command => {
            let command = check
                .command
                .as_deref()
                .ok_or_else(|| "command check is missing command".to_string())?;
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output()
                .await
                .map_err(|error| format!("failed to run command check: {error}"))?;
            let status = if output.status.success() {
                "passed"
            } else {
                "failed"
            };
            Ok((
                status.to_string(),
                format!(
                    "command `{}` exited with {}",
                    command,
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string())
                ),
                output.status.code(),
            ))
        }
        CheckKind::Presence | CheckKind::Absence => {
            let path = check
                .path
                .as_ref()
                .or(check.file.as_ref())
                .ok_or_else(|| "presence/absence check is missing path or file".to_string())?;
            let pattern = check
                .pattern
                .as_deref()
                .ok_or_else(|| "presence/absence check is missing pattern".to_string())?;
            let full_path = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let content = fs::read_to_string(&full_path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let contains = content.contains(pattern);
            let passed = match check.kind {
                CheckKind::Presence => contains,
                CheckKind::Absence => !contains,
                _ => unreachable!(),
            };
            let status = if passed { "passed" } else { "failed" };
            let expectation = if matches!(check.kind, CheckKind::Presence) {
                "contains"
            } else {
                "does not contain"
            };
            Ok((
                status.to_string(),
                format!("{} {} `{}`", path.display(), expectation, pattern),
                None,
            ))
        }
        CheckKind::ChangedFiles => {
            if check.paths.is_empty() {
                return Err("changed_files check is missing paths".to_string());
            }
            let mut command = tokio::process::Command::new("git");
            command
                .arg("status")
                .arg("--porcelain")
                .arg("--")
                .args(&check.paths)
                .current_dir(cwd);
            let output = command
                .output()
                .await
                .map_err(|error| format!("failed to inspect git status: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "git status failed with {}",
                    output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string())
                ));
            }
            let changed = !String::from_utf8_lossy(&output.stdout).trim().is_empty();
            let status = if changed { "passed" } else { "failed" };
            Ok((
                status.to_string(),
                format!(
                    "changed files check inspected {} path(s)",
                    check.paths.len()
                ),
                output.status.code(),
            ))
        }
        _ => Err(format!("check kind {:?} is not runnable", check.kind)),
    }
}
