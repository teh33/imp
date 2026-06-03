use std::path::{Path, PathBuf};

use serde_json::json;

use super::workflow_files::{
    load_selected_workflow, load_workflow_items, validate_loaded_workflow, workflow_paths,
};
use super::workflow_render::render_workflow;
use super::{ToolOutput, WorkflowDiagnosticView, WorkflowListItem, WorkflowValidationModeParam};
use crate::error::Result;
use crate::workflow::{load_workflow, validate_workflow};

pub(super) fn workflows_root(cwd: &Path) -> PathBuf {
    cwd.join(".imp").join("workflows")
}

pub(super) fn list_action(workflows_root: &Path) -> Result<ToolOutput> {
    let workflows = load_workflow_items(workflows_root)?;
    if workflows.is_empty() {
        return Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text {
                text: "No workflows found under .imp/workflows.".to_string(),
            }],
            details: json!({ "action": "list", "workflows": Vec::<WorkflowListItem>::new() }),
            is_error: false,
        });
    }

    let mut text = String::from("Workflows:\n");
    for workflow in &workflows {
        text.push_str(&format!(
            "- {} [{}] {}\n",
            workflow.id, workflow.status, workflow.title
        ));
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({ "action": "list", "workflows": workflows }),
        is_error: false,
    })
}

pub(super) fn show_action(
    workflows_root: &Path,
    id: Option<&str>,
    mode: WorkflowValidationModeParam,
) -> Result<ToolOutput> {
    let (id, root, doc) = load_selected_workflow(workflows_root, id)?;
    let diagnostics = validate_workflow(&doc, &mode.options(root));
    let text = render_workflow(&id, &doc, &diagnostics);
    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({
            "action": "show",
            "id": id,
            "diagnostics": diagnostics.iter().map(|diagnostic| WorkflowDiagnosticView {
                path: diagnostic.path.clone(),
                message: diagnostic.message.clone(),
            }).collect::<Vec<_>>()
        }),
        is_error: false,
    })
}

pub(super) fn validate_action(
    workflows_root: &Path,
    id: Option<&str>,
    mode: WorkflowValidationModeParam,
) -> Result<ToolOutput> {
    let results = if let Some(id) = id {
        let (_, root, doc) = load_selected_workflow(workflows_root, Some(id))?;
        vec![validate_loaded_workflow(&doc, &root, mode)]
    } else {
        let mut results = Vec::new();
        for path in workflow_paths(workflows_root)? {
            let root = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| workflows_root.to_path_buf());
            let doc = load_workflow(&path).map_err(|error| {
                crate::error::Error::Tool(format!("failed to load {}: {error}", path.display()))
            })?;
            results.push(validate_loaded_workflow(&doc, &root, mode));
        }
        results
    };

    let ok_count = results.iter().filter(|result| result.ok).count();
    let mut text = format!(
        "Validated {} workflow(s): {} ok, {} with diagnostics.",
        results.len(),
        ok_count,
        results.len().saturating_sub(ok_count)
    );
    for result in &results {
        if result.ok {
            text.push_str(&format!("\n- {}: ok", result.id));
        } else {
            text.push_str(&format!("\n- {}: diagnostics", result.id));
            for diagnostic in &result.diagnostics {
                text.push_str(&format!(
                    "\n  - {}: {}",
                    diagnostic.path, diagnostic.message
                ));
            }
        }
    }

    Ok(ToolOutput {
        content: vec![imp_llm::ContentBlock::Text { text }],
        details: json!({ "action": "validate", "results": results }),
        is_error: false,
    })
}
