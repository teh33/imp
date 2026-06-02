use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const WORKFLOW_SCHEMA_VERSION: &str = "imp.workflow/v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDocument {
    pub schema: String,
    pub id: String,
    pub title: String,
    pub status: WorkflowStatus,
    pub kind: String,
    #[serde(default)]
    pub parent: Option<WorkflowParent>,
    #[serde(default)]
    pub settings: BTreeMap<String, serde_yaml::Value>,
    pub spec: WorkflowSpec,
    #[serde(default)]
    pub context: BTreeMap<String, ContextRequirement>,
    pub steps: BTreeMap<String, WorkflowStep>,
    #[serde(default)]
    pub prototypes: BTreeMap<String, WorkflowPrototype>,
    pub checks: BTreeMap<String, WorkflowCheck>,
    pub workers: BTreeMap<String, WorkflowWorker>,
    pub results: WorkflowResults,
    pub closeout: WorkflowCloseout,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowParent {
    pub workflow: String,
    pub step: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    pub goal: String,
    #[serde(default)]
    pub user_value: Option<String>,
    #[serde(default)]
    pub non_goals: Vec<String>,
    pub acceptance: BTreeMap<String, AcceptanceCriterion>,
    #[serde(default)]
    pub approval_required_for: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub text: String,
    pub status: AcceptanceStatus,
    #[serde(default)]
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequirement {
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub artifact: Option<PathBuf>,
    #[serde(default)]
    pub search: Option<String>,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    pub kind: StepKind,
    pub status: StepStatus,
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub prototypes: Vec<String>,
    #[serde(default)]
    pub action: Option<WorkflowStepAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepAction {
    pub kind: WorkflowStepActionKind,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub worker: Option<String>,
    pub objective: String,
    #[serde(default)]
    pub instructions: Vec<String>,
    #[serde(default)]
    pub write_scope: Vec<PathBuf>,
    #[serde(default)]
    pub completion: WorkflowStepActionCompletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepActionKind {
    Agent,
    Worker,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepActionCompletion {
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Spec,
    Context,
    Plan,
    Prototype,
    Build,
    Verify,
    Review,
    Workflow,
    Decision,
    Closeout,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPrototype {
    pub question: String,
    #[serde(default)]
    pub hypothesis: Option<String>,
    pub status: PrototypeStatus,
    #[serde(default)]
    pub criteria: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCheck {
    pub kind: CheckKind,
    pub status: CheckStatus,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub file: Option<PathBuf>,
    #[serde(default)]
    pub artifact: Option<PathBuf>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub broad: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Context,
    Artifact,
    Command,
    Review,
    Approval,
    Aggregate,
    Closeout,
    ChangedFiles,
    Presence,
    Absence,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowWorker {
    pub role: String,
    #[serde(default)]
    pub writes: Vec<String>,
    #[serde(default)]
    pub writes_code: Option<bool>,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub responsibilities: Vec<String>,
    #[serde(default)]
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowResults {
    pub path: PathBuf,
    #[serde(default)]
    pub required_claims: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCloseout {
    pub done: CloseoutDone,
    #[serde(default)]
    pub allowed_terminal_statuses: Vec<WorkflowStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseoutDone {
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Planned,
    Active,
    Waiting,
    Blocked,
    Done,
    DoneWithConcerns,
    NeedsContext,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Todo,
    Ready,
    Active,
    Waiting,
    Blocked,
    Done,
    DoneWithConcerns,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pending,
    Passed,
    Failed,
    Blocked,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrototypeStatus {
    Proposed,
    Active,
    Supported,
    Refuted,
    Inconclusive,
    Selected,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceStatus {
    Todo,
    Done,
    Blocked,
    NeedsContext,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    Draft,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateOptions {
    pub mode: ValidationMode,
    pub workflow_root: PathBuf,
}

impl ValidateOptions {
    pub fn strict(workflow_root: impl Into<PathBuf>) -> Self {
        Self {
            mode: ValidationMode::Strict,
            workflow_root: workflow_root.into(),
        }
    }

    pub fn draft(workflow_root: impl Into<PathBuf>) -> Self {
        Self {
            mode: ValidationMode::Draft,
            workflow_root: workflow_root.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDiagnostic {
    pub path: String,
    pub message: String,
}

impl WorkflowDiagnostic {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

const MAX_WORKFLOW_YAML_BYTES: u64 = 1_048_576;

#[derive(Debug)]
pub enum WorkflowLoadError {
    Io(std::io::Error),
    TooLarge { size: u64, limit: u64 },
    Yaml(serde_yaml::Error),
}

impl fmt::Display for WorkflowLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkflowLoadError::Io(error) => write!(f, "failed to read workflow: {error}"),
            WorkflowLoadError::TooLarge { size, limit } => write!(
                f,
                "failed to read workflow: workflow.yaml is {size} bytes, above the {limit} byte limit"
            ),
            WorkflowLoadError::Yaml(error) => write!(f, "failed to parse workflow YAML: {error}"),
        }
    }
}

impl std::error::Error for WorkflowLoadError {}

impl From<std::io::Error> for WorkflowLoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_yaml::Error> for WorkflowLoadError {
    fn from(error: serde_yaml::Error) -> Self {
        Self::Yaml(error)
    }
}

pub fn load_workflow_raw(path: &Path) -> Result<String, WorkflowLoadError> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    if size > MAX_WORKFLOW_YAML_BYTES {
        return Err(WorkflowLoadError::TooLarge {
            size,
            limit: MAX_WORKFLOW_YAML_BYTES,
        });
    }
    Ok(fs::read_to_string(path)?)
}

pub fn load_workflow(path: &Path) -> Result<WorkflowDocument, WorkflowLoadError> {
    let raw = load_workflow_raw(path)?;
    Ok(serde_yaml::from_str(&raw)?)
}

pub fn validate_workflow(
    doc: &WorkflowDocument,
    options: &ValidateOptions,
) -> Vec<WorkflowDiagnostic> {
    let mut diagnostics = Vec::new();

    if doc.schema != WORKFLOW_SCHEMA_VERSION {
        diagnostics.push(WorkflowDiagnostic::new(
            "schema",
            format!("unsupported workflow schema `{}`", doc.schema),
        ));
    }

    if doc.spec.goal.trim().is_empty() {
        diagnostics.push(WorkflowDiagnostic::new(
            "spec.goal",
            "goal must not be empty",
        ));
    }

    validate_directory_id(doc, options, &mut diagnostics);
    validate_step_references(doc, options, &mut diagnostics);
    validate_check_references(doc, &mut diagnostics);
    validate_check_contracts(doc, &mut diagnostics);
    validate_implementation_step_evidence(doc, &mut diagnostics);
    validate_acceptance(doc, &mut diagnostics);
    validate_closeout(doc, &mut diagnostics);
    validate_action_contracts(doc, &mut diagnostics);
    validate_parent_link(doc, options, &mut diagnostics);
    validate_passed_artifacts(doc, options, &mut diagnostics);
    validate_step_cycles(doc, &mut diagnostics);
    validate_check_cycles(doc, &mut diagnostics);

    diagnostics
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowStepReadiness {
    pub step: String,
    pub status: StepStatus,
    pub state: WorkflowReadinessState,
    pub reasons: Vec<WorkflowReadinessReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowReadinessState {
    Runnable,
    Waiting,
    Blocked,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowReadinessReason {
    pub kind: WorkflowReadinessReasonKind,
    pub subject: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowReadinessReasonKind {
    DependencyMissing,
    DependencyNotReady,
    WorkerMissing,
    StatusNotRunnable,
    CheckPending,
    CheckFailed,
    CheckBlocked,
}

pub fn workflow_step_readiness(doc: &WorkflowDocument) -> Vec<WorkflowStepReadiness> {
    doc.steps
        .iter()
        .map(|(step_id, step)| {
            let mut reasons = Vec::new();

            if is_terminal_step_status(step.status) {
                return WorkflowStepReadiness {
                    step: step_id.clone(),
                    status: step.status,
                    state: WorkflowReadinessState::Terminal,
                    reasons,
                };
            }

            if !matches!(step.status, StepStatus::Todo | StepStatus::Ready) {
                reasons.push(WorkflowReadinessReason {
                    kind: WorkflowReadinessReasonKind::StatusNotRunnable,
                    subject: Some(step_id.clone()),
                    message: format!("step status is {}", format!("{:?}", step.status).to_case()),
                });
            }

            for dependency in &step.depends_on {
                match doc.steps.get(dependency) {
                    Some(dependency_step) if is_terminal_success(dependency_step.status) => {}
                    Some(dependency_step) => reasons.push(WorkflowReadinessReason {
                        kind: WorkflowReadinessReasonKind::DependencyNotReady,
                        subject: Some(dependency.clone()),
                        message: format!(
                            "dependency `{dependency}` is {}",
                            format!("{:?}", dependency_step.status).to_case()
                        ),
                    }),
                    None => reasons.push(WorkflowReadinessReason {
                        kind: WorkflowReadinessReasonKind::DependencyMissing,
                        subject: Some(dependency.clone()),
                        message: format!("dependency `{dependency}` is missing"),
                    }),
                }
            }

            if let Some(worker) = &step.worker {
                if !doc.workers.contains_key(worker) {
                    reasons.push(WorkflowReadinessReason {
                        kind: WorkflowReadinessReasonKind::WorkerMissing,
                        subject: Some(worker.clone()),
                        message: format!("worker `{worker}` is missing"),
                    });
                }
            }

            for check_id in &step.checks {
                if let Some(check) = doc.checks.get(check_id) {
                    let kind = match check.status {
                        CheckStatus::Pending => Some(WorkflowReadinessReasonKind::CheckPending),
                        CheckStatus::Failed => Some(WorkflowReadinessReasonKind::CheckFailed),
                        CheckStatus::Blocked => Some(WorkflowReadinessReasonKind::CheckBlocked),
                        CheckStatus::Passed | CheckStatus::Skipped => None,
                    };
                    if let Some(kind) = kind {
                        reasons.push(WorkflowReadinessReason {
                            kind,
                            subject: Some(check_id.clone()),
                            message: format!(
                                "check `{check_id}` is {}",
                                format!("{:?}", check.status).to_case()
                            ),
                        });
                    }
                }
            }

            let state = if reasons.iter().any(|reason| {
                matches!(
                    reason.kind,
                    WorkflowReadinessReasonKind::DependencyMissing
                        | WorkflowReadinessReasonKind::WorkerMissing
                )
            }) {
                WorkflowReadinessState::Blocked
            } else if reasons.iter().any(|reason| {
                matches!(
                    reason.kind,
                    WorkflowReadinessReasonKind::DependencyNotReady
                        | WorkflowReadinessReasonKind::StatusNotRunnable
                )
            }) {
                WorkflowReadinessState::Waiting
            } else {
                WorkflowReadinessState::Runnable
            };

            WorkflowStepReadiness {
                step: step_id.clone(),
                status: step.status,
                state,
                reasons,
            }
        })
        .collect()
}

pub fn next_runnable_steps(doc: &WorkflowDocument) -> Vec<String> {
    workflow_step_readiness(doc)
        .into_iter()
        .filter(|readiness| matches!(readiness.state, WorkflowReadinessState::Runnable))
        .map(|readiness| readiness.step)
        .collect()
}

fn is_terminal_step_status(status: StepStatus) -> bool {
    matches!(
        status,
        StepStatus::Done
            | StepStatus::DoneWithConcerns
            | StepStatus::Skipped
            | StepStatus::Failed
            | StepStatus::Blocked
    )
}

fn validate_directory_id(
    doc: &WorkflowDocument,
    options: &ValidateOptions,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    if options.mode != ValidationMode::Strict {
        return;
    }

    let Some(directory_name) = options
        .workflow_root
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return;
    };

    if directory_name != doc.id {
        diagnostics.push(WorkflowDiagnostic::new(
            "id",
            format!(
                "workflow id `{}` should match directory `{directory_name}`",
                doc.id
            ),
        ));
    }
}

fn validate_step_references(
    doc: &WorkflowDocument,
    options: &ValidateOptions,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    for (step_id, step) in &doc.steps {
        for dependency in &step.depends_on {
            if !doc.steps.contains_key(dependency) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.depends_on"),
                    format!("unknown step `{dependency}`"),
                ));
            }
        }

        for check in &step.checks {
            if !doc.checks.contains_key(check) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.checks"),
                    format!("unknown check `{check}`"),
                ));
            }
        }

        for prototype in &step.prototypes {
            if !doc.prototypes.contains_key(prototype) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.prototypes"),
                    format!("unknown prototype `{prototype}`"),
                ));
            }
        }

        if let Some(worker) = &step.worker {
            if !doc.workers.contains_key(worker) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.worker"),
                    format!("unknown worker `{worker}`"),
                ));
            }
        }

        if matches!(step.kind, StepKind::Workflow) {
            match &step.workflow {
                Some(workflow) => {
                    if !is_workflow_directory_name(workflow) {
                        diagnostics.push(WorkflowDiagnostic::new(
                            format!("steps.{step_id}.workflow"),
                            format!("workflow `{workflow}` must be a workflow directory name"),
                        ));
                        continue;
                    }
                    if options.mode == ValidationMode::Strict {
                        let child_path = options
                            .workflow_root
                            .parent()
                            .unwrap_or(options.workflow_root.as_path())
                            .join(workflow)
                            .join("workflow.yaml");
                        if !child_path.exists() {
                            diagnostics.push(WorkflowDiagnostic::new(
                                format!("steps.{step_id}.workflow"),
                                format!(
                                    "workflow `{workflow}` does not exist at {}",
                                    child_path.display()
                                ),
                            ));
                        }
                    }
                }
                None => diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.workflow"),
                    "workflow step must reference a workflow id",
                )),
            }
        }
    }
}

fn validate_action_contracts(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    for (step_id, step) in &doc.steps {
        let Some(action) = &step.action else {
            continue;
        };

        if action.objective.trim().is_empty() {
            diagnostics.push(WorkflowDiagnostic::new(
                format!("steps.{step_id}.action.objective"),
                "action objective must not be empty",
            ));
        }

        if matches!(action.kind, WorkflowStepActionKind::Worker) {
            match action.worker.as_deref() {
                Some(worker) if doc.workers.contains_key(worker) => {}
                Some(worker) => diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.action.worker"),
                    format!("unknown worker `{worker}`"),
                )),
                None => diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.action.worker"),
                    "worker action must reference a worker",
                )),
            }
        }

        for check in &action.completion.checks {
            if !doc.checks.contains_key(check) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("steps.{step_id}.action.completion.checks"),
                    format!("unknown check `{check}`"),
                ));
            }
        }
    }
}

fn validate_check_references(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    for (check_id, check) in &doc.checks {
        for dependency in &check.requires {
            if !doc.checks.contains_key(dependency) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("checks.{check_id}.requires"),
                    format!("unknown check `{dependency}`"),
                ));
            }
        }
    }
}

fn validate_check_contracts(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    for (check_id, check) in &doc.checks {
        match check.kind {
            CheckKind::Presence | CheckKind::Absence => {
                if check.path.is_none() && check.file.is_none() {
                    diagnostics.push(WorkflowDiagnostic::new(
                        format!("checks.{check_id}.path"),
                        "presence/absence check must include path or file",
                    ));
                }
                if check
                    .pattern
                    .as_deref()
                    .is_none_or(|pattern| pattern.trim().is_empty())
                {
                    diagnostics.push(WorkflowDiagnostic::new(
                        format!("checks.{check_id}.pattern"),
                        "presence/absence check must include a non-empty pattern",
                    ));
                }
            }
            CheckKind::ChangedFiles => {
                if check.paths.is_empty() {
                    diagnostics.push(WorkflowDiagnostic::new(
                        format!("checks.{check_id}.paths"),
                        "changed_files check must include at least one path",
                    ));
                }
            }
            _ => {}
        }
    }
}

fn validate_implementation_step_evidence(
    doc: &WorkflowDocument,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    if doc.kind != "implementation" {
        return;
    }

    for (step_id, step) in &doc.steps {
        if !matches!(step.kind, StepKind::Build) {
            continue;
        }
        if step
            .checks
            .iter()
            .filter_map(|check_id| doc.checks.get(check_id))
            .any(is_implementation_evidence_check)
        {
            continue;
        }
        diagnostics.push(WorkflowDiagnostic::new(
            format!("steps.{step_id}.checks"),
            "implementation build step must include change-sensitive implementation evidence; broad command checks alone are not sufficient",
        ));
    }
}

fn is_implementation_evidence_check(check: &WorkflowCheck) -> bool {
    matches!(
        check.kind,
        CheckKind::Artifact | CheckKind::ChangedFiles | CheckKind::Presence | CheckKind::Absence
    ) || (matches!(check.kind, CheckKind::Command) && !check.broad)
}

fn validate_acceptance(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    for (criterion_id, criterion) in &doc.spec.acceptance {
        if criterion.text.trim().is_empty() {
            diagnostics.push(WorkflowDiagnostic::new(
                format!("spec.acceptance.{criterion_id}.text"),
                "acceptance text must not be empty",
            ));
        }

        for check in &criterion.checks {
            if !doc.checks.contains_key(check) {
                diagnostics.push(WorkflowDiagnostic::new(
                    format!("spec.acceptance.{criterion_id}.checks"),
                    format!("unknown check `{check}`"),
                ));
            }
        }
    }
}

fn validate_closeout(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    if doc.closeout.done.requires.is_empty() {
        diagnostics.push(WorkflowDiagnostic::new(
            "closeout.done.requires",
            "clean done must require at least one check or built-in predicate",
        ));
    }

    for requirement in &doc.closeout.done.requires {
        if is_builtin_closeout_predicate(requirement) {
            continue;
        }
        if !doc.checks.contains_key(requirement) {
            diagnostics.push(WorkflowDiagnostic::new(
                "closeout.done.requires",
                format!("unknown check or built-in predicate `{requirement}`"),
            ));
        }
    }
}

fn validate_parent_link(
    doc: &WorkflowDocument,
    options: &ValidateOptions,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    if options.mode != ValidationMode::Strict {
        return;
    }

    let Some(parent) = &doc.parent else {
        return;
    };

    if !is_workflow_directory_name(&parent.workflow) {
        diagnostics.push(WorkflowDiagnostic::new(
            "parent.workflow",
            format!(
                "parent workflow `{}` must be a workflow directory name",
                parent.workflow
            ),
        ));
        return;
    }

    let Some(workflows_dir) = options.workflow_root.parent() else {
        return;
    };

    let parent_path = workflows_dir.join(&parent.workflow).join("workflow.yaml");
    let Ok(parent_doc) = load_workflow(&parent_path) else {
        diagnostics.push(WorkflowDiagnostic::new(
            "parent.workflow",
            format!("parent workflow `{}` not found", parent.workflow),
        ));
        return;
    };

    match parent_doc.steps.get(&parent.step) {
        Some(step)
            if matches!(step.kind, StepKind::Workflow)
                && step.workflow.as_deref() == Some(&doc.id) => {}
        Some(_) => diagnostics.push(WorkflowDiagnostic::new(
            "parent.step",
            format!(
                "parent step `{}` does not call workflow `{}`",
                parent.step, doc.id
            ),
        )),
        None => diagnostics.push(WorkflowDiagnostic::new(
            "parent.step",
            format!("parent step `{}` not found", parent.step),
        )),
    }
}

fn validate_passed_artifacts(
    doc: &WorkflowDocument,
    options: &ValidateOptions,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    if options.mode != ValidationMode::Strict {
        return;
    }

    for (check_id, check) in &doc.checks {
        if !matches!(check.kind, CheckKind::Artifact) || check.status != CheckStatus::Passed {
            continue;
        }
        let Some(path) = check
            .path
            .as_ref()
            .or(check.artifact.as_ref())
            .or(check.file.as_ref())
        else {
            diagnostics.push(WorkflowDiagnostic::new(
                format!("checks.{check_id}.path"),
                "passed artifact check must include path, artifact, or file",
            ));
            continue;
        };
        let resolved_path = resolve_project_path(path, options);
        if !resolved_path.exists() {
            diagnostics.push(WorkflowDiagnostic::new(
                format!("checks.{check_id}.path"),
                format!("artifact `{}` does not exist", path.display()),
            ));
        }
    }
}

fn resolve_project_path(path: &Path, options: &ValidateOptions) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    project_root_from_workflow_root(&options.workflow_root)
        .unwrap_or_else(|| options.workflow_root.clone())
        .join(path)
}

fn project_root_from_workflow_root(workflow_root: &Path) -> Option<PathBuf> {
    // Expected shape: <project>/.imp/workflows/<workflow-id>
    workflow_root
        .parent()?
        .parent()?
        .parent()
        .map(Path::to_path_buf)
}

fn validate_step_cycles(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    let graph: BTreeMap<&str, Vec<&str>> = doc
        .steps
        .iter()
        .map(|(id, step)| {
            (
                id.as_str(),
                step.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    validate_acyclic_graph("steps", &graph, diagnostics);
}

fn validate_check_cycles(doc: &WorkflowDocument, diagnostics: &mut Vec<WorkflowDiagnostic>) {
    let graph: BTreeMap<&str, Vec<&str>> = doc
        .checks
        .iter()
        .map(|(id, check)| {
            (
                id.as_str(),
                check.requires.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    validate_acyclic_graph("checks", &graph, diagnostics);
}

fn validate_acyclic_graph(
    label: &str,
    graph: &BTreeMap<&str, Vec<&str>>,
    diagnostics: &mut Vec<WorkflowDiagnostic>,
) {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();

    for node in graph.keys() {
        if has_cycle(node, graph, &mut visiting, &mut visited) {
            diagnostics.push(WorkflowDiagnostic::new(
                label,
                format!("{label} dependency graph contains a cycle involving `{node}`"),
            ));
            return;
        }
    }
}

fn has_cycle<'a>(
    node: &'a str,
    graph: &BTreeMap<&'a str, Vec<&'a str>>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> bool {
    if visited.contains(node) {
        return false;
    }
    if !visiting.insert(node) {
        return true;
    }

    for dependency in graph.get(node).into_iter().flatten() {
        if graph.contains_key(dependency) && has_cycle(dependency, graph, visiting, visited) {
            return true;
        }
    }

    visiting.remove(node);
    visited.insert(node);
    false
}

fn is_terminal_success(status: StepStatus) -> bool {
    matches!(status, StepStatus::Done | StepStatus::DoneWithConcerns)
}

fn is_builtin_closeout_predicate(value: &str) -> bool {
    matches!(value, "no_unapproved_goal_or_acceptance_changes")
}

fn is_workflow_directory_name(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
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

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn workflow_root(id: &str) -> PathBuf {
        repo_root().join(".imp/workflows").join(id)
    }

    fn load_fixture(id: &str) -> WorkflowDocument {
        load_workflow(&workflow_root(id).join("workflow.yaml")).expect("fixture should load")
    }

    fn validate_fixture(id: &str) -> Vec<WorkflowDiagnostic> {
        let doc = load_fixture(id);
        validate_workflow(&doc, &ValidateOptions::strict(workflow_root(id)))
    }

    #[test]
    fn workflow_schema_dogfood_workflows_parse_and_validate() {
        for id in [
            "prototype-imp-workflow-engine",
            "define-workflow-schema",
            "prototype-rust-workflow-schema-parser",
        ] {
            let diagnostics = validate_fixture(id);
            assert_eq!(
                diagnostics,
                Vec::new(),
                "{id} diagnostics: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn workflow_schema_reference_validation_rejects_missing_refs() {
        let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
        doc.steps
            .get_mut("add_schema_module")
            .expect("step exists")
            .depends_on
            .push("missing_step".to_owned());
        doc.steps
            .get_mut("add_schema_module")
            .expect("step exists")
            .checks
            .push("missing_check".to_owned());
        doc.steps
            .get_mut("add_schema_module")
            .expect("step exists")
            .worker = Some("missing_worker".to_owned());
        doc.steps
            .get_mut("record_parser_considerations")
            .expect("step exists")
            .prototypes
            .push("missing_prototype".to_owned());
        doc.closeout
            .done
            .requires
            .push("missing_closeout_check".to_owned());

        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::draft(workflow_root("prototype-rust-workflow-schema-parser")),
        );
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            messages.contains("unknown step `missing_step`"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("unknown check `missing_check`"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("unknown worker `missing_worker`"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("unknown prototype `missing_prototype`"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("unknown check or built-in predicate `missing_closeout_check`"),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn workflow_schema_shape_validation_rejects_bad_status_and_acceptance() {
        let yaml = r#"
schema: imp.workflow/v1
id: broken
title: Broken
status: active
kind: test
settings: {}
spec:
  goal: Broken workflow.
  acceptance:
    missing_status:
      text: This should fail.
context: {}
steps:
  first:
    kind: context
    status: nope
prototypes: {}
checks: {}
workers: {}
results:
  path: .imp/workflows/broken/results.md
closeout:
  done:
    requires: []
"#;

        let error = serde_yaml::from_str::<WorkflowDocument>(yaml)
            .expect_err("invalid status and missing acceptance status should fail during parse");
        let message = error.to_string();
        assert!(message.contains("status"), "{message}");
    }

    #[test]
    fn workflow_readiness_explains_runnable_and_blocked_steps() {
        let yaml = r#"
schema: imp.workflow/v1
id: readiness
title: Readiness
status: active
kind: test
settings: {}
spec:
  goal: Test readiness.
  acceptance:
    done:
      text: Work is ready.
      status: todo
context: {}
steps:
  inspect:
    kind: context
    status: done
  build:
    kind: build
    status: todo
    depends_on: [inspect]
    worker: missing_builder
    checks: [pending_check]
  verify:
    kind: verify
    status: todo
    depends_on: [build]
  missing_dep:
    kind: verify
    status: todo
    depends_on: [does_not_exist]
  active_step:
    kind: build
    status: active
  failed_step:
    kind: build
    status: failed
prototypes: {}
checks:
  pending_check:
    kind: command
    status: pending
    command: cargo test -p imp-core workflow
workers: {}
results:
  path: .imp/workflows/readiness/results.md
closeout:
  done:
    requires: [pending_check]
"#;
        let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
        let readiness = workflow_step_readiness(&doc);
        let find = |step: &str| {
            readiness
                .iter()
                .find(|entry| entry.step == step)
                .unwrap_or_else(|| panic!("missing readiness for {step}"))
        };

        assert_eq!(find("inspect").state, WorkflowReadinessState::Terminal);
        assert_eq!(find("build").state, WorkflowReadinessState::Blocked);
        assert!(find("build").reasons.iter().any(|reason| {
            reason.kind == WorkflowReadinessReasonKind::WorkerMissing
                && reason.subject.as_deref() == Some("missing_builder")
        }));
        assert!(find("build").reasons.iter().any(|reason| {
            reason.kind == WorkflowReadinessReasonKind::CheckPending
                && reason.subject.as_deref() == Some("pending_check")
        }));
        assert_eq!(find("verify").state, WorkflowReadinessState::Waiting);
        assert!(find("verify").reasons.iter().any(|reason| {
            reason.kind == WorkflowReadinessReasonKind::DependencyNotReady
                && reason.subject.as_deref() == Some("build")
        }));
        assert_eq!(find("missing_dep").state, WorkflowReadinessState::Blocked);
        assert!(find("missing_dep").reasons.iter().any(|reason| {
            reason.kind == WorkflowReadinessReasonKind::DependencyMissing
                && reason.subject.as_deref() == Some("does_not_exist")
        }));
        assert_eq!(find("active_step").state, WorkflowReadinessState::Waiting);
        assert!(find("active_step")
            .reasons
            .iter()
            .any(|reason| { reason.kind == WorkflowReadinessReasonKind::StatusNotRunnable }));
        assert_eq!(find("failed_step").state, WorkflowReadinessState::Terminal);
    }

    #[test]
    fn workflow_readiness_preserves_next_runnable_steps_compatibility() {
        let yaml = r#"
schema: imp.workflow/v1
id: readiness-compat
title: Readiness compatibility
status: active
kind: test
settings: {}
spec:
  goal: Test next runnable compatibility.
  acceptance:
    done:
      text: Work is ready.
      status: todo
context: {}
steps:
  add_schema_module:
    kind: build
    status: todo
  add_validation_tests:
    kind: verify
    status: todo
    depends_on: [add_schema_module]
prototypes: {}
checks: {}
workers: {}
results:
  path: .imp/workflows/readiness-compat/results.md
closeout:
  done:
    requires: []
"#;
        let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");

        let readiness = workflow_step_readiness(&doc)
            .into_iter()
            .filter(|entry| matches!(entry.state, WorkflowReadinessState::Runnable))
            .map(|entry| entry.step)
            .collect::<Vec<_>>();

        assert_eq!(readiness, vec!["add_schema_module".to_owned()]);
        assert_eq!(next_runnable_steps(&doc), readiness);
    }

    #[test]
    fn workflow_schema_next_runnable_steps_respects_dependencies() {
        let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
        doc.steps
            .get_mut("add_schema_module")
            .expect("step exists")
            .status = StepStatus::Todo;
        doc.steps
            .get_mut("add_validation_tests")
            .expect("step exists")
            .status = StepStatus::Todo;

        assert_eq!(
            next_runnable_steps(&doc),
            vec!["add_schema_module".to_owned()]
        );
    }

    #[test]
    fn evidence_quality_check_kinds() {
        let yaml = r#"
schema: imp.workflow/v1
id: evidence-quality
title: Evidence Quality
status: active
kind: implementation
settings: {}
spec:
  goal: Validate evidence check kinds.
  acceptance:
    done:
      text: Evidence is checked.
      status: todo
      checks: [source_changed, classifier_removed, prompt_present]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [source_changed, classifier_removed, prompt_present]
prototypes: {}
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [crates/imp-core/src/workflow/schema.rs]
  classifier_removed:
    kind: absence
    status: pending
    path: crates/imp-core/src/agent/mod.rs
    pattern: classify_stop_reason_from_text
  prompt_present:
    kind: presence
    status: pending
    file: crates/imp-core/src/system_prompt.rs
    pattern: Tool routing
workers: {}
results:
  path: .imp/workflows/evidence-quality/results.md
closeout:
  done:
    requires: [source_changed]
"#;
        let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::draft(PathBuf::from(".imp/workflows/evidence-quality")),
        );
        assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");

        let broken_yaml = yaml
            .replace(
                "paths: [crates/imp-core/src/workflow/schema.rs]",
                "paths: []",
            )
            .replace("pattern: classify_stop_reason_from_text", "pattern: ''")
            .replace(
                "file: crates/imp-core/src/system_prompt.rs",
                "question: missing file",
            );
        let broken_doc: WorkflowDocument =
            serde_yaml::from_str(&broken_yaml).expect("workflow parses");
        let diagnostics = validate_workflow(
            &broken_doc,
            &ValidateOptions::draft(PathBuf::from(".imp/workflows/evidence-quality")),
        );
        let messages = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            messages.contains("changed_files check must include at least one path"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("presence/absence check must include a non-empty pattern"),
            "{diagnostics:#?}"
        );
        assert!(
            messages.contains("presence/absence check must include path or file"),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn weak_implementation_checks_are_rejected() {
        let yaml = r#"
schema: imp.workflow/v1
id: weak-implementation
title: Weak Implementation
status: active
kind: implementation
settings: {}
spec:
  goal: Reject broad checks alone.
  acceptance:
    done:
      text: Work is done.
      status: todo
      checks: [broad_tests]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [broad_tests]
prototypes: {}
checks:
  broad_tests:
    kind: command
    status: pending
    broad: true
    command: cargo test -p imp-core --lib
workers: {}
results:
  path: .imp/workflows/weak-implementation/results.md
closeout:
  done:
    requires: [broad_tests]
"#;
        let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::draft(PathBuf::from(".imp/workflows/weak-implementation")),
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path == "steps.implement.checks"
                    && diagnostic.message.contains("broad command checks alone")),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn implementation_steps_require_change_sensitive_evidence() {
        let yaml = r#"
schema: imp.workflow/v1
id: strong-implementation
title: Strong Implementation
status: active
kind: implementation
settings: {}
spec:
  goal: Accept change-sensitive evidence.
  acceptance:
    done:
      text: Work is done.
      status: todo
      checks: [source_changed, broad_tests]
context: {}
steps:
  implement:
    kind: build
    status: todo
    checks: [source_changed, broad_tests]
prototypes: {}
checks:
  source_changed:
    kind: changed_files
    status: pending
    paths: [crates/imp-core/src/workflow/schema.rs]
  broad_tests:
    kind: command
    status: pending
    broad: true
    command: cargo test -p imp-core --lib
workers: {}
results:
  path: .imp/workflows/strong-implementation/results.md
closeout:
  done:
    requires: [broad_tests]
"#;
        let doc: WorkflowDocument = serde_yaml::from_str(yaml).expect("workflow parses");
        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::draft(PathBuf::from(".imp/workflows/strong-implementation")),
        );
        assert_eq!(diagnostics, Vec::new(), "{diagnostics:#?}");
    }

    #[test]
    fn workflow_schema_strict_validation_rejects_parent_mismatch() {
        let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
        doc.parent.as_mut().expect("parent exists").step = "plan_rust_validator".to_owned();

        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::strict(workflow_root("prototype-rust-workflow-schema-parser")),
        );

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("does not call workflow")),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn workflow_schema_load_rejects_oversized_workflow_yaml() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = temp.path().join("workflow.yaml");
        std::fs::write(&path, vec![b'a'; (MAX_WORKFLOW_YAML_BYTES + 1) as usize])
            .expect("write oversized workflow");

        let error = load_workflow(&path).expect_err("oversized workflow should fail before parse");
        let message = error.to_string();
        assert!(message.contains("above the"), "{message}");
        assert!(message.contains("byte limit"), "{message}");
    }

    #[test]
    fn workflow_schema_rejects_path_like_workflow_references() {
        let mut doc = load_fixture("define-workflow-schema");
        doc.steps
            .get_mut("prototype_rust_parser")
            .expect("workflow step exists")
            .workflow = Some("../prototype-rust-workflow-schema-parser".to_owned());

        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::strict(workflow_root("define-workflow-schema")),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.path == "steps.prototype_rust_parser.workflow"
                    && diagnostic
                        .message
                        .contains("must be a workflow directory name")
            }),
            "{diagnostics:#?}"
        );

        let mut doc = load_fixture("prototype-rust-workflow-schema-parser");
        doc.parent.as_mut().expect("parent exists").workflow = "/tmp/parent".to_owned();

        let diagnostics = validate_workflow(
            &doc,
            &ValidateOptions::strict(workflow_root("prototype-rust-workflow-schema-parser")),
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.path == "parent.workflow"
                    && diagnostic
                        .message
                        .contains("must be a workflow directory name")
            }),
            "{diagnostics:#?}"
        );
    }
}
