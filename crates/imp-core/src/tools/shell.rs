use std::fmt;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::process::{ProcessError, ProcessManager};
use crate::tools::{truncate_head, truncate_tail, Tool, ToolContext, ToolOutput, ToolRegistry};

mod params;
mod runtime;

use params::{interpolate_arg, validate_required_params};

const MAX_OUTPUT_LINES: usize = 2000;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

/// TOML-defined shell tool definition.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellToolDef {
    pub name: String,
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub params: std::collections::HashMap<String, ShellParamDef>,
    pub exec: ShellExecDef,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellParamDef {
    #[serde(rename = "type")]
    pub param_type: String,
    pub description: String,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellExecDef {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    #[serde(default = "default_truncate")]
    pub truncate: String,
    pub install_hint: Option<String>,
}

#[derive(Clone)]
pub struct ShellTool {
    def: ShellToolDef,
    manager: ProcessManager,
}

impl fmt::Debug for ShellTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShellTool")
            .field("def", &self.def)
            .finish()
    }
}

impl ShellTool {
    #[cfg(test)]
    fn new(def: ShellToolDef) -> Self {
        Self::with_manager(def, ProcessManager::new())
    }

    fn with_manager(def: ShellToolDef, manager: ProcessManager) -> Self {
        Self { def, manager }
    }
}

fn default_timeout() -> u32 {
    30
}
fn default_truncate() -> String {
    "head".into()
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn label(&self) -> &str {
        &self.def.label
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn parameters(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();

        let mut param_names: Vec<_> = self.def.params.keys().cloned().collect();
        param_names.sort();

        for name in param_names {
            if let Some(def) = self.def.params.get(&name) {
                properties.insert(
                    name.clone(),
                    json!({
                        "type": def.param_type,
                        "description": def.description,
                    }),
                );

                if !def.optional {
                    required.push(Value::String(name));
                }
            }
        }

        json!({
            "type": "object",
            "properties": Value::Object(properties),
            "required": Value::Array(required),
        })
    }

    fn is_readonly(&self) -> bool {
        self.def.readonly
    }

    async fn execute(&self, _call_id: &str, params: Value, ctx: ToolContext) -> Result<ToolOutput> {
        if ctx.is_cancelled() {
            return Ok(ToolOutput::error("Tool execution cancelled."));
        }

        let provided = params.as_object().cloned().unwrap_or_default();
        validate_required_params(&self.def.params, &provided)?;

        let mut args = Vec::with_capacity(self.def.exec.args.len());
        for arg in &self.def.exec.args {
            args.push(interpolate_arg(arg, &self.def.params, &provided)?);
        }

        let execution = match runtime::execute(&self.manager, &self.def, args, &ctx).await {
            Ok(execution) => execution,
            Err(ProcessError::Spawn(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(command_not_found_output(&self.def));
            }
            Err(error) => {
                return Err(Error::Tool(format!(
                    "shell tool '{}' failed: {error}",
                    self.def.name
                )));
            }
        };
        let mut combined_output = execution.stdout;
        if !execution.stderr.is_empty() {
            if !combined_output.is_empty() && !combined_output.ends_with('\n') {
                combined_output.push('\n');
            }
            combined_output.push_str(&execution.stderr);
        }

        let truncation = match self.def.exec.truncate.as_str() {
            "tail" => truncate_tail(&combined_output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES),
            _ => truncate_head(&combined_output, MAX_OUTPUT_LINES, MAX_OUTPUT_BYTES),
        };

        let mut result_text = truncation.content;
        if truncation.truncated {
            let note = format!(
                "\n[Output truncated: showing {} of {} lines{}]",
                truncation.output_lines,
                truncation.total_lines,
                truncation
                    .temp_file
                    .as_ref()
                    .map(|path| format!(". Full output saved to {}", path.display()))
                    .unwrap_or_default()
            );
            result_text.push_str(&note);
        }
        let timed_out = execution.exit.timed_out;
        let cancelled = execution.exit.cancelled;
        let exit_code = execution.exit.code.unwrap_or(-1);
        if timed_out {
            result_text.push_str(&format!(
                "\n[Command timed out after {}s]",
                self.def.exec.timeout
            ));
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text: result_text }],
            details: json!({
                "exit_code": exit_code,
                "timed_out": timed_out,
                "cancelled": cancelled,
                "truncated": truncation.truncated,
            }),
            is_error: timed_out || cancelled || exit_code != 0,
        })
    }
}

fn command_not_found_output(def: &ShellToolDef) -> ToolOutput {
    let mut message = format!(
        "Command not found for shell tool '{}': {}",
        def.name, def.exec.command
    );
    if let Some(hint) = &def.exec.install_hint {
        message.push_str(&format!("\nInstall hint: {hint}"));
    }
    ToolOutput::error(message)
}

/// Load shell tools from a directory of TOML definitions.
pub fn load_shell_tools(dir: &Path, registry: &mut ToolRegistry) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    let manager = ProcessManager::new();
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.map_err(|err| {
            Error::Tool(format!(
                "failed to walk shell tool directory {}: {err}",
                dir.display()
            ))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }

        let content = std::fs::read_to_string(entry.path())?;
        match toml::from_str::<ShellToolDef>(&content) {
            Ok(def) => registry.register(Arc::new(ShellTool::with_manager(def, manager.clone()))),
            Err(_err) => {
                // Keep shell tool discovery side-effect free for embedded callers.
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;
