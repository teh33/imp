mod anchored;
mod exact;
mod matching;
mod output;

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolOutput};
use crate::error::Result;
pub(crate) use matching::apply_edit;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn label(&self) -> &str {
        "Edit File"
    }

    fn description(&self) -> &str {
        "Edit files by exact replacement, anchored range, or edits[] transaction."
    }

    fn parameters(&self) -> serde_json::Value {
        edit_schema()
    }

    fn is_readonly(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        call_id: &str,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolOutput> {
        execute_edit(call_id, params, ctx).await
    }
}

async fn execute_edit(
    call_id: &str,
    params: serde_json::Value,
    ctx: ToolContext,
) -> Result<ToolOutput> {
    if use_transaction_mode(&params) {
        return super::multi_edit::MultiEditTool
            .execute(call_id, params, ctx)
            .await;
    }
    let raw_path = params["path"].as_str().unwrap_or("");
    if raw_path.is_empty() {
        return Ok(ToolOutput::error("Missing required parameter: path"));
    }
    let path = super::resolve_path(&ctx.cwd, raw_path);
    let anchor_start =
        get_str_param(&params, "anchor_start", "anchorStart").filter(|anchor| !anchor.is_empty());
    if anchor_start.is_some() {
        return anchored::execute(&path, raw_path, &params, ctx).await;
    }
    exact::execute(&path, raw_path, &params, ctx).await
}

fn use_transaction_mode(params: &serde_json::Value) -> bool {
    let edits = params.get("edits").and_then(|value| value.as_array());
    let has_edits = edits.is_some_and(|edits| !edits.is_empty());
    let has_single_fields = ["path", "old_text", "oldText", "anchor_start", "anchorStart"]
        .iter()
        .any(|name| non_empty_string(params, name));
    has_edits || (edits.is_some() && !has_single_fields)
}

fn non_empty_string(params: &serde_json::Value, name: &str) -> bool {
    params
        .get(name)
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty())
}

pub(super) fn get_str_param<'a>(
    params: &'a serde_json::Value,
    primary: &str,
    legacy: &str,
) -> Option<&'a str> {
    params
        .get(primary)
        .and_then(|value| value.as_str())
        .or_else(|| params.get(legacy).and_then(|value| value.as_str()))
}

pub(super) fn get_bool_param(
    params: &serde_json::Value,
    primary: &str,
    legacy: &str,
) -> Option<bool> {
    params
        .get(primary)
        .and_then(|value| value.as_bool())
        .or_else(|| params.get(legacy).and_then(|value| value.as_bool()))
}

fn edit_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path or default edits[] path" },
            "old_text": { "type": "string", "description": "Text to replace" },
            "new_text": { "type": "string", "description": "Replacement text" },
            "dry_run": { "type": "boolean", "description": "Dry run; return diff only" },
            "expected_occurrences": {
                "type": "integer", "description": "Required exact old_text match count"
            },
            "replace_all": { "type": "boolean", "description": "Replace all exact matches" },
            "anchor_start": {
                "type": "string", "description": "Start anchor from read(anchors=true)"
            },
            "anchor_end": { "type": "string", "description": "Optional end anchor" },
            "target": {
                "type": "string",
                "description": "Optional target symbol guard. old_text must occur inside this symbol/block for parseable source files."
            },
            "validate_syntax": {
                "type": "boolean",
                "description": "When true, parse the edited source and report syntax errors before apply/during dry run."
            },
            "edits": {
                "type": "array",
                "description": "Transactional edits[]",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Per-edit path" },
                        "old_text": { "type": "string" },
                        "new_text": { "type": "string" }
                    },
                    "required": ["old_text", "new_text"]
                }
            }
        },
        "required": []
    })
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
