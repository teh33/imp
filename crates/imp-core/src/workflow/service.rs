use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    load_workflow, validate_workflow, ValidateOptions, ValidationMode, WorkflowDiagnostic,
    WorkflowDocument,
};

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct WorkflowReadResult {
    pub id: String,
    pub workflow: WorkflowDocument,
    pub diagnostics: Vec<WorkflowDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowEventRecord {
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowServiceRunResult {
    pub id: String,
    pub status: String,
    pub execution_mode: WorkflowServiceExecutionMode,
    pub next_action: WorkflowServiceNextAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowServiceExecutionMode {
    MainAgent,
    Subagents,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowServiceNextAction {
    OrchestratedCommandChecks {
        steps: Vec<WorkflowServiceCommandStepRun>,
        reconciled: Vec<String>,
    },
    AgentAction {
        step: String,
        step_kind: String,
        contract: WorkflowServiceAgentActionContract,
    },
    MissingActionContract {
        step: String,
        step_kind: String,
        reason: String,
    },
    NoRunnableSteps {
        blocked_steps: Vec<WorkflowServiceBlockedStep>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowServiceCommandStepRun {
    pub step: String,
    pub step_status: String,
    pub checks: Vec<WorkflowServiceCommandCheckRun>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowServiceCommandCheckRun {
    pub check: String,
    pub command: String,
    pub status: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowServiceAgentActionContract {
    pub workflow_id: String,
    pub step: String,
    pub step_kind: String,
    pub role: String,
    pub objective: String,
    pub instructions: Vec<String>,
    pub write_scope: Vec<String>,
    pub completion_checks: Vec<String>,
    pub completion_artifacts: Vec<String>,
    pub worker: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowServiceBlockedStep {
    pub step: String,
    pub status: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowService {
    workflows_root: PathBuf,
}

impl WorkflowService {
    pub fn new(workflows_root: impl Into<PathBuf>) -> Self {
        Self {
            workflows_root: workflows_root.into(),
        }
    }

    pub fn workflows_root(&self) -> &Path {
        &self.workflows_root
    }

    pub fn list(&self) -> crate::error::Result<Vec<WorkflowSummary>> {
        let mut items = Vec::new();
        for path in self.workflow_paths()? {
            let doc = load_workflow(&path).map_err(|error| {
                crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
            })?;
            items.push(WorkflowSummary {
                id: doc.id,
                title: doc.title,
                status: format!("{:?}", doc.status).to_case(),
                kind: format!("{:?}", doc.kind).to_case(),
                path,
            });
        }
        Ok(items)
    }

    pub fn read(&self, id: &str, mode: ValidationMode) -> crate::error::Result<WorkflowReadResult> {
        let workflow_root = self.workflow_root(id)?;
        let path = workflow_root.join("workflow.yaml");
        let workflow = load_workflow(&path).map_err(|error| {
            crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
        })?;
        let diagnostics = validate_workflow(
            &workflow,
            &ValidateOptions {
                mode,
                workflow_root,
            },
        );
        Ok(WorkflowReadResult {
            id: workflow.id.clone(),
            workflow,
            diagnostics,
        })
    }

    pub fn validate(
        &self,
        id: Option<&str>,
        mode: ValidationMode,
    ) -> crate::error::Result<Vec<WorkflowReadResult>> {
        if let Some(id) = id {
            return Ok(vec![self.read(id, mode)?]);
        }

        let mut results = Vec::new();
        for path in self.workflow_paths()? {
            let workflow_root = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.workflows_root.clone());
            let workflow = load_workflow(&path).map_err(|error| {
                crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
            })?;
            let diagnostics = validate_workflow(
                &workflow,
                &ValidateOptions {
                    mode,
                    workflow_root,
                },
            );
            results.push(WorkflowReadResult {
                id: workflow.id.clone(),
                workflow,
                diagnostics,
            });
        }
        Ok(results)
    }

    pub fn events(
        &self,
        id: &str,
        limit: Option<usize>,
    ) -> crate::error::Result<Vec<WorkflowEventRecord>> {
        let path = self.workflow_root(id)?.join("events.jsonl");
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(crate::error::Error::Tool(format!(
                    "failed to read {}: {error}",
                    path.display()
                )));
            }
        };
        let mut events = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .map(|value| WorkflowEventRecord { value })
                    .map_err(|error| {
                        crate::error::Error::Tool(format!(
                            "failed to parse event in {}: {error}",
                            path.display()
                        ))
                    })
            })
            .collect::<crate::error::Result<Vec<_>>>()?;
        if let Some(limit) = limit {
            if events.len() > limit {
                events = events.split_off(events.len() - limit);
            }
        }
        Ok(events)
    }

    pub fn update_status(
        &self,
        id: &str,
        update_path: &str,
        value: &str,
        reason: &str,
        check_write_path: impl Fn(&Path) -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        let workflow_root = self.workflow_root(id)?;
        let workflow_path = workflow_root.join("workflow.yaml");
        let event_path = workflow_root.join("events.jsonl");
        let tmp_path = workflow_path.with_extension("yaml.tmp");
        check_write_path(&workflow_path)?;
        check_write_path(&event_path)?;
        check_write_path(&tmp_path)?;

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
        apply_status_update(&mut yaml, update_path, value)?;
        self.write_validated_workflow(&workflow_root, &workflow_path, &tmp_path, &yaml)?;
        append_event(&event_path, "update", update_path, value, reason)
    }

    pub fn complete_step(
        &self,
        id: &str,
        step_id: &str,
        reason: &str,
        check_write_path: impl Fn(&Path) -> crate::error::Result<()>,
    ) -> crate::error::Result<Vec<String>> {
        let workflow_root = self.workflow_root(id)?;
        let workflow_path = workflow_root.join("workflow.yaml");
        let event_path = workflow_root.join("events.jsonl");
        let tmp_path = workflow_path.with_extension("yaml.tmp");
        check_write_path(&workflow_path)?;
        check_write_path(&event_path)?;
        check_write_path(&tmp_path)?;

        let raw = fs::read_to_string(&workflow_path).map_err(|error| {
            crate::error::Error::Tool(format!(
                "failed to read {}: {error}",
                workflow_path.display()
            ))
        })?;
        let doc: WorkflowDocument = serde_yaml::from_str(&raw).map_err(|error| {
            crate::error::Error::Tool(format!(
                "failed to parse {}: {error}",
                workflow_path.display()
            ))
        })?;
        let step = doc.steps.get(step_id).ok_or_else(|| {
            crate::error::Error::Tool(format!("unknown workflow step `{step_id}`"))
        })?;
        let mut yaml: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|error| {
            crate::error::Error::Tool(format!(
                "failed to parse {}: {error}",
                workflow_path.display()
            ))
        })?;

        set_nested_mapping_string(&mut yaml, &["steps", step_id], "status", "done")?;
        append_event(
            &event_path,
            "complete_step",
            &format!("steps.{step_id}.status"),
            "done",
            reason,
        )?;
        let mut completed_checks = Vec::new();
        for check_id in &step.checks {
            if doc.checks.contains_key(check_id) {
                set_nested_mapping_string(&mut yaml, &["checks", check_id], "status", "passed")?;
                append_event(
                    &event_path,
                    "complete_step",
                    &format!("checks.{check_id}.status"),
                    "passed",
                    &format!("step `{step_id}` completed: {reason}"),
                )?;
                completed_checks.push(check_id.clone());
            }
        }
        reconcile_statuses(&mut yaml, &doc, &event_path)?;
        self.write_validated_workflow(&workflow_root, &workflow_path, &tmp_path, &yaml)?;
        Ok(completed_checks)
    }

    fn write_validated_workflow(
        &self,
        workflow_root: &Path,
        workflow_path: &Path,
        tmp_path: &Path,
        yaml: &serde_yaml::Value,
    ) -> crate::error::Result<()> {
        let updated = serde_yaml::to_string(yaml).map_err(|error| {
            crate::error::Error::Tool(format!("failed to serialize workflow: {error}"))
        })?;
        let candidate: WorkflowDocument = serde_yaml::from_str(&updated).map_err(|error| {
            crate::error::Error::Tool(format!(
                "workflow update would produce invalid YAML/schema: {error}"
            ))
        })?;
        let diagnostics = validate_workflow(
            &candidate,
            &ValidateOptions::strict(workflow_root.to_path_buf()),
        );
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
        fs::write(tmp_path, updated).map_err(|error| {
            crate::error::Error::Tool(format!("failed to write {}: {error}", tmp_path.display()))
        })?;
        fs::rename(tmp_path, workflow_path).map_err(|error| {
            crate::error::Error::Tool(format!(
                "failed to replace {} with {}: {error}",
                workflow_path.display(),
                tmp_path.display()
            ))
        })
    }

    fn workflow_paths(&self) -> crate::error::Result<Vec<PathBuf>> {
        if !self.workflows_root.exists() {
            return Ok(Vec::new());
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.workflows_root).map_err(|error| {
            crate::error::Error::Tool(format!(
                "failed to read {}: {error}",
                self.workflows_root.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                crate::error::Error::Tool(format!(
                    "failed to read {} entry: {error}",
                    self.workflows_root.display()
                ))
            })?;
            let path = entry.path().join("workflow.yaml");
            if path.exists() {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    fn workflow_root(&self, id: &str) -> crate::error::Result<PathBuf> {
        let mut components = Path::new(id).components();
        let Some(std::path::Component::Normal(_)) = components.next() else {
            return Err(invalid_workflow_id(id));
        };
        if components.next().is_some() {
            return Err(invalid_workflow_id(id));
        }
        Ok(self.workflows_root.join(id))
    }
}

fn invalid_workflow_id(id: &str) -> crate::error::Error {
    crate::error::Error::Tool(format!(
        "invalid workflow id `{id}`; expected a workflow directory name under .imp/workflows"
    ))
}

fn apply_status_update(
    yaml: &mut serde_yaml::Value,
    update_path: &str,
    value: &str,
) -> crate::error::Result<()> {
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

fn set_mapping_string(
    yaml: &mut serde_yaml::Value,
    key: &str,
    value: &str,
) -> crate::error::Result<()> {
    let Some(mapping) = yaml.as_mapping_mut() else {
        return Err(crate::error::Error::Tool(
            "workflow root is not a mapping".into(),
        ));
    };
    mapping.insert(
        serde_yaml::Value::String(key.to_string()),
        serde_yaml::Value::String(value.to_string()),
    );
    Ok(())
}

fn set_nested_mapping_string(
    yaml: &mut serde_yaml::Value,
    path: &[&str],
    key: &str,
    value: &str,
) -> crate::error::Result<()> {
    let mut current = yaml;
    for part in path {
        current = current.get_mut(*part).ok_or_else(|| {
            crate::error::Error::Tool(format!("workflow path component `{part}` not found"))
        })?;
    }
    let Some(mapping) = current.as_mapping_mut() else {
        return Err(crate::error::Error::Tool(format!(
            "workflow path `{}` is not a mapping",
            path.join(".")
        )));
    };
    mapping.insert(
        serde_yaml::Value::String(key.to_string()),
        serde_yaml::Value::String(value.to_string()),
    );
    Ok(())
}

fn append_event(
    path: &Path,
    action: &str,
    update_path: &str,
    value: &str,
    reason: &str,
) -> crate::error::Result<()> {
    let event = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "action": action,
        "path": update_path,
        "value": value,
        "reason": reason,
    });
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            crate::error::Error::Tool(format!("failed to open {}: {error}", path.display()))
        })?;
    use std::io::Write as _;
    writeln!(file, "{}", event).map_err(|error| {
        crate::error::Error::Tool(format!("failed to append {}: {error}", path.display()))
    })
}

fn reconcile_statuses(
    yaml: &mut serde_yaml::Value,
    doc: &WorkflowDocument,
    event_path: &Path,
) -> crate::error::Result<()> {
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
            set_nested_mapping_string(
                yaml,
                &["spec", "acceptance", acceptance_id],
                "status",
                "done",
            )?;
            append_event(
                event_path,
                "reconcile",
                &format!("spec.acceptance.{acceptance_id}.status"),
                "done",
                "all acceptance checks passed",
            )?;
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
        append_event(
            event_path,
            "reconcile",
            "status",
            "done",
            "closeout requirements passed and all steps are terminal",
        )?;
    }
    Ok(())
}

trait CaseExt {
    fn to_case(&self) -> String;
}

impl CaseExt for str {
    fn to_case(&self) -> String {
        let mut out = String::new();
        for (index, ch) in self.chars().enumerate() {
            if ch.is_uppercase() && index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_workflow(root: &Path) -> PathBuf {
        let workflows_root = root.join(".imp/workflows");
        let workflow_root = workflows_root.join("service-fixture");
        fs::create_dir_all(&workflow_root).expect("create workflow root");
        fs::write(
            workflow_root.join("workflow.yaml"),
            r#"schema: imp.workflow/v1
id: service-fixture
title: Service Fixture
status: active
kind: implementation
spec:
  goal: Exercise workflow service.
  acceptance:
    checked:
      text: Service works.
      status: todo
      checks: [checked]
steps:
  check:
    kind: verify
    status: todo
    checks: [checked]
checks:
  checked:
    kind: command
    status: pending
    command: true
results:
  path: .imp/workflows/service-fixture/results.md
workers: {}
closeout:
  done:
    requires: [checked]
"#,
        )
        .expect("write workflow");
        fs::write(
            workflow_root.join("events.jsonl"),
            "{\"action\":\"update\",\"path\":\"status\",\"value\":\"active\",\"reason\":\"fixture\"}\n",
        )
        .expect("write events");
        workflows_root
    }

    #[test]
    fn workflow_service_lists_reads_validates_and_reads_events() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_workflow(temp.path());
        let service = WorkflowService::new(&workflows_root);

        let list = service.list().expect("list workflows");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "service-fixture");
        assert_eq!(list[0].status, "active");

        let read = service
            .read("service-fixture", ValidationMode::Strict)
            .expect("read workflow");
        assert_eq!(read.id, "service-fixture");
        assert!(read.diagnostics.is_empty(), "{:?}", read.diagnostics);

        let validated = service
            .validate(Some("service-fixture"), ValidationMode::Strict)
            .expect("validate workflow");
        assert_eq!(validated.len(), 1);
        assert!(validated[0].diagnostics.is_empty());

        let events = service
            .events("service-fixture", Some(1))
            .expect("read events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value["action"], "update");
    }

    #[test]
    fn workflow_service_updates_and_completes_steps_with_events() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let workflows_root = write_workflow(temp.path());
        let service = WorkflowService::new(&workflows_root);
        let allow_write = |_path: &Path| Ok(());

        service
            .update_status(
                "service-fixture",
                "steps.check.status",
                "ready",
                "prepare step",
                allow_write,
            )
            .expect("update status");
        assert!(matches!(
            service
                .read("service-fixture", ValidationMode::Strict)
                .expect("read updated workflow")
                .workflow
                .steps
                .get("check")
                .expect("step exists")
                .status,
            crate::workflow::StepStatus::Ready
        ));

        let completed = service
            .complete_step(
                "service-fixture",
                "check",
                "service completion test",
                allow_write,
            )
            .expect("complete step");
        assert_eq!(completed, vec!["checked".to_string()]);
        let read = service
            .read("service-fixture", ValidationMode::Strict)
            .expect("read completed workflow");
        assert!(matches!(
            read.workflow
                .steps
                .get("check")
                .expect("step exists")
                .status,
            crate::workflow::StepStatus::Done
        ));
        assert!(matches!(
            read.workflow
                .checks
                .get("checked")
                .expect("check exists")
                .status,
            crate::workflow::CheckStatus::Passed
        ));
        assert!(matches!(
            read.workflow.status,
            crate::workflow::WorkflowStatus::Done
        ));
        let events = service
            .events("service-fixture", None)
            .expect("events readable");
        assert!(events
            .iter()
            .any(|event| event.value["action"] == "complete_step"));
        assert!(events
            .iter()
            .any(|event| event.value["action"] == "reconcile"));
    }

    #[test]
    fn workflow_service_run_model_serializes_for_transports() {
        let result = WorkflowServiceRunResult {
            id: "service-fixture".to_string(),
            status: "active".to_string(),
            execution_mode: WorkflowServiceExecutionMode::MainAgent,
            next_action: WorkflowServiceNextAction::AgentAction {
                step: "check".to_string(),
                step_kind: "verify".to_string(),
                contract: WorkflowServiceAgentActionContract {
                    workflow_id: "service-fixture".to_string(),
                    step: "check".to_string(),
                    step_kind: "verify".to_string(),
                    role: "verifier".to_string(),
                    objective: "Verify the workflow service.".to_string(),
                    instructions: vec!["Inspect the service result.".to_string()],
                    write_scope: vec![".imp/workflows/service-fixture/results.md".to_string()],
                    completion_checks: vec!["checked".to_string()],
                    completion_artifacts: vec![
                        ".imp/workflows/service-fixture/results.md".to_string()
                    ],
                    worker: None,
                },
            },
        };

        let value = serde_json::to_value(result).expect("serializes");
        assert_eq!(value["id"], "service-fixture");
        assert_eq!(value["execution_mode"], "main_agent");
        assert_eq!(value["next_action"]["kind"], "agent_action");
        assert_eq!(
            value["next_action"]["contract"]["completion_checks"][0],
            "checked"
        );
    }

    #[test]
    fn workflow_service_rejects_nested_workflow_ids() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let service = WorkflowService::new(temp.path().join(".imp/workflows"));
        let error = service
            .read("../escape", ValidationMode::Strict)
            .unwrap_err();
        assert!(error.to_string().contains("invalid workflow id"));
    }
}
