use std::path::{Path, PathBuf};

use super::render::CaseExt;
use super::{
    WorkflowDiagnosticView, WorkflowListItem, WorkflowValidationModeParam, WorkflowValidationResult,
};
use crate::error::Result;
use crate::workflow::{load_workflow, validate_workflow, WorkflowDocument};

pub(super) fn load_workflow_items(workflows_root: &Path) -> Result<Vec<WorkflowListItem>> {
    let mut items = Vec::new();
    for path in workflow_paths(workflows_root)? {
        let doc = load_workflow(&path).map_err(|error| {
            crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
        })?;
        items.push(WorkflowListItem {
            id: doc.id,
            title: doc.title,
            status: format!("{:?}", doc.status).to_case(),
            kind: doc.kind,
            path,
        });
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(items)
}

pub(super) fn workflow_paths(workflows_root: &Path) -> Result<Vec<PathBuf>> {
    if !workflows_root.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(workflows_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("workflow.yaml");
        if path.exists() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(super) fn workflow_id_root(workflows_root: &Path, id: &str) -> Result<PathBuf> {
    let mut components = Path::new(id).components();
    let Some(std::path::Component::Normal(_)) = components.next() else {
        return Err(invalid_workflow_id(id));
    };
    if components.next().is_some() {
        return Err(invalid_workflow_id(id));
    }
    Ok(workflows_root.join(id))
}

fn invalid_workflow_id(id: &str) -> crate::error::Error {
    crate::error::Error::Tool(format!(
        "invalid workflow id `{id}`; expected a workflow directory name under .imp/workflows"
    ))
}

pub(super) fn load_selected_workflow(
    workflows_root: &Path,
    id: Option<&str>,
) -> Result<(String, PathBuf, WorkflowDocument)> {
    let path = if let Some(id) = id {
        workflow_id_root(workflows_root, id)?.join("workflow.yaml")
    } else {
        let paths = workflow_paths(workflows_root)?;
        match paths.as_slice() {
            [path] => path.clone(),
            [] => {
                return Err(crate::error::Error::Tool(
                    "no workflows found under .imp/workflows".into(),
                ));
            }
            _ => {
                return Err(crate::error::Error::Tool(
                    "multiple workflows found; provide `id`".into(),
                ));
            }
        }
    };

    let root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workflows_root.to_path_buf());
    let doc = load_workflow(&path).map_err(|error| {
        crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
    })?;
    let id = doc.id.clone();
    Ok((id, root, doc))
}

pub(super) fn validate_loaded_workflow(
    doc: &WorkflowDocument,
    root: &Path,
    mode: WorkflowValidationModeParam,
) -> WorkflowValidationResult {
    let diagnostics = validate_workflow(doc, &mode.options(root.to_path_buf()))
        .into_iter()
        .map(|diagnostic| WorkflowDiagnosticView {
            path: diagnostic.path,
            message: diagnostic.message,
        })
        .collect::<Vec<_>>();
    WorkflowValidationResult {
        id: doc.id.clone(),
        ok: diagnostics.is_empty(),
        diagnostics,
    }
}
