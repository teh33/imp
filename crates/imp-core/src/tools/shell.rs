use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::child_process::{isolate_tokio_command, kill_tokio_process_group};

use crate::error::{Error, Result};
use crate::tools::{truncate_head, truncate_tail, Tool, ToolContext, ToolOutput, ToolRegistry};

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

#[derive(Debug, Clone)]
pub struct ShellTool {
    def: ShellToolDef,
}

impl ShellTool {
    fn new(def: ShellToolDef) -> Self {
        Self { def }
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

        let mut command = Command::new(&self.def.exec.command);
        command
            .args(&args)
            .current_dir(&ctx.cwd)
            // Shell tools are also non-interactive; avoid inheriting the TUI's
            // stdin so terminal escape sequences cannot leak into child reads.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Create a new process group so timeout cleanup can terminate
        // grandchildren spawned by TOML-defined shell tools too.
        isolate_tokio_command(&mut command);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut message = format!(
                    "Command not found for shell tool '{}': {}",
                    self.def.name, self.def.exec.command
                );
                if let Some(hint) = &self.def.exec.install_hint {
                    message.push_str(&format!("\nInstall hint: {hint}"));
                }
                return Ok(ToolOutput::error(message));
            }
            Err(err) => {
                return Err(Error::Tool(format!(
                    "failed to spawn shell tool '{}': {err}",
                    self.def.name
                )));
            }
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Tool("failed to capture stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Tool("failed to capture stderr".into()))?;

        let stdout_task = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stdout);
            let mut buffer = Vec::new();
            reader.read_to_end(&mut buffer).await.map(|_| buffer)
        });
        let stderr_task = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr);
            let mut buffer = Vec::new();
            reader.read_to_end(&mut buffer).await.map(|_| buffer)
        });

        let timeout = std::time::Duration::from_secs(self.def.exec.timeout as u64);
        let (status, timed_out) = tokio::select! {
            status = child.wait() => (status?, false),
            _ = tokio::time::sleep(timeout) => {
                kill_tokio_process_group(&child).await;
                let _ = child.kill().await;
                let status = child.wait().await?;
                (status, true)
            }
        };

        let stdout_bytes = stdout_task
            .await
            .map_err(|err| Error::Tool(format!("stdout reader task failed: {err}")))??;
        let stderr_bytes = stderr_task
            .await
            .map_err(|err| Error::Tool(format!("stderr reader task failed: {err}")))??;

        let mut combined_output = String::new();
        let stdout_text = String::from_utf8_lossy(&stdout_bytes);
        let stderr_text = String::from_utf8_lossy(&stderr_bytes);

        if !stdout_text.is_empty() {
            combined_output.push_str(&stdout_text);
        }
        if !stderr_text.is_empty() {
            if !combined_output.is_empty() && !combined_output.ends_with('\n') {
                combined_output.push('\n');
            }
            combined_output.push_str(&stderr_text);
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
        if timed_out {
            result_text.push_str(&format!(
                "\n[Command timed out after {}s]",
                self.def.exec.timeout
            ));
        }

        Ok(ToolOutput {
            content: vec![imp_llm::ContentBlock::Text { text: result_text }],
            details: json!({
                "exit_code": status.code().unwrap_or(-1),
                "timed_out": timed_out,
                "truncated": truncation.truncated,
            }),
            is_error: timed_out || !status.success(),
        })
    }
}

fn validate_required_params(
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<()> {
    let mut missing = Vec::new();

    for (name, def) in defs {
        if !def.optional && provided.get(name).is_none_or(Value::is_null) {
            missing.push(name.clone());
        }
    }

    missing.sort();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::Tool(format!(
            "missing required parameter(s): {}",
            missing.join(", ")
        )))
    }
}

fn interpolate_arg(
    template: &str,
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<String> {
    let mut result = String::new();
    let mut remaining = template;

    while let Some(start) = remaining.find('{') {
        result.push_str(&remaining[..start]);

        let after_start = &remaining[start + 1..];
        let end = after_start.find('}').ok_or_else(|| {
            Error::Tool(format!(
                "unclosed placeholder in shell tool argument: {template}"
            ))
        })?;

        let placeholder = &after_start[..end];
        result.push_str(&resolve_placeholder(placeholder, defs, provided)?);
        remaining = &after_start[end + 1..];
    }

    result.push_str(remaining);
    Ok(result)
}

fn resolve_placeholder(
    placeholder: &str,
    defs: &HashMap<String, ShellParamDef>,
    provided: &Map<String, Value>,
) -> Result<String> {
    let (name, default) = placeholder
        .split_once('|')
        .map_or((placeholder, None), |(name, default)| (name, Some(default)));

    if name.is_empty() {
        return Err(Error::Tool(
            "empty placeholder in shell tool argument".into(),
        ));
    }

    if let Some(value) = provided.get(name).filter(|value| !value.is_null()) {
        return stringify_param_value(name, value);
    }

    if let Some(default) = default {
        return Ok(default.to_string());
    }

    if defs.get(name).is_some_and(|def| def.optional) {
        return Ok(String::new());
    }

    Err(Error::Tool(format!(
        "missing required parameter for placeholder: {name}"
    )))
}

fn stringify_param_value(name: &str, value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        _ => Err(Error::Tool(format!(
            "parameter '{name}' must be a string, number, or boolean"
        ))),
    }
}

/// Load shell tools from a directory of TOML definitions.
pub fn load_shell_tools(dir: &Path, registry: &mut ToolRegistry) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

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
            Ok(def) => registry.register(Arc::new(ShellTool::new(def))),
            Err(_err) => {
                // Keep shell tool discovery side-effect free for embedded callers.
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::NullInterface;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn test_ctx(dir: &Path) -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(16);
        ToolContext {
            cwd: dir.to_path_buf(),
            cancelled: Arc::new(AtomicBool::new(false)),
            update_tx: tx,
            command_tx: cmd_tx,
            ui: Arc::new(NullInterface),
            file_cache: Arc::new(crate::tools::FileCache::new()),
            checkpoint_state: Arc::new(crate::tools::CheckpointState::new()),
            file_tracker: Arc::new(std::sync::Mutex::new(crate::tools::FileTracker::new())),
            anchor_store: Arc::new(crate::tools::AnchorStore::new()),
            lua_tool_loader: None,
            mode: crate::config::AgentMode::Full,
            read_max_lines: 500,
            turn_workflow_review: Arc::new(std::sync::Mutex::new(
                crate::workflow_review::TurnWorkflowReviewAccumulator::default(),
            )),
            config: Arc::new(crate::config::Config::default()),
            run_policy: Default::default(),
            supporting_provenance: Vec::new(),
        }
    }

    #[test]
    fn load_shell_tools_registers_valid_defs_and_skips_invalid_ones() {
        let temp_dir = tempfile::tempdir().unwrap();
        let tools_dir = temp_dir.path().join("tools");
        std::fs::create_dir_all(tools_dir.join("nested")).unwrap();

        std::fs::write(
            tools_dir.join("nested").join("greet.toml"),
            r#"
name = "greet"
label = "Greet"
description = "Print a greeting"
readonly = true

[params.name]
type = "string"
description = "Name to greet"

[params.greeting]
type = "string"
description = "Greeting text"
optional = true

[exec]
command = "printf"
args = ["%s %s", "{greeting|hello}", "{name}"]
timeout = 5
truncate = "head"
"#,
        )
        .unwrap();

        std::fs::write(tools_dir.join("broken.toml"), "not = [valid").unwrap();

        let mut registry = ToolRegistry::new();
        load_shell_tools(&tools_dir, &mut registry).unwrap();

        let tool = registry.get("greet").expect("tool should be registered");
        assert_eq!(tool.name(), "greet");
        assert!(registry.get("broken").is_none());
    }

    #[tokio::test]
    async fn shell_tool_executes_with_param_interpolation() {
        let tool = ShellTool::new(ShellToolDef {
            name: "greet".into(),
            label: "Greet".into(),
            description: "Print a greeting".into(),
            readonly: true,
            params: HashMap::from([
                (
                    "name".into(),
                    ShellParamDef {
                        param_type: "string".into(),
                        description: "Name to greet".into(),
                        optional: false,
                    },
                ),
                (
                    "greeting".into(),
                    ShellParamDef {
                        param_type: "string".into(),
                        description: "Greeting text".into(),
                        optional: true,
                    },
                ),
            ]),
            exec: ShellExecDef {
                command: "printf".into(),
                args: vec!["%s %s".into(), "{greeting|hello}".into(), "{name}".into()],
                timeout: 5,
                truncate: "head".into(),
                install_hint: None,
            },
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let result = tool
            .execute(
                "call-1",
                json!({ "name": "Asher" }),
                test_ctx(temp_dir.path()),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = match &result.content[0] {
            imp_llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        };
        assert_eq!(text, "hello Asher");
        assert_eq!(result.details["exit_code"], 0);
        assert_eq!(result.details["timed_out"], false);
    }

    #[tokio::test]
    async fn shell_tool_default_param_used_when_not_provided() {
        let tool = ShellTool::new(ShellToolDef {
            name: "echo_default".into(),
            label: "Echo Default".into(),
            description: "Echo with default".into(),
            readonly: true,
            params: HashMap::from([(
                "msg".into(),
                ShellParamDef {
                    param_type: "string".into(),
                    description: "Message".into(),
                    optional: true,
                },
            )]),
            exec: ShellExecDef {
                command: "echo".into(),
                args: vec!["{msg|default_value}".into()],
                timeout: 5,
                truncate: "head".into(),
                install_hint: None,
            },
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let result = tool
            .execute("call-3", json!({}), test_ctx(temp_dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = match &result.content[0] {
            imp_llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        };
        assert!(text.contains("default_value"));
    }

    #[test]
    fn shell_tool_required_param_missing_errors() {
        let defs = HashMap::from([(
            "name".into(),
            ShellParamDef {
                param_type: "string".into(),
                description: "Name".into(),
                optional: false,
            },
        )]);
        let provided = serde_json::Map::new();
        let result = validate_required_params(&defs, &provided);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("name"));
    }

    #[tokio::test]
    async fn shell_tool_stderr_included_in_output() {
        let tool = ShellTool::new(ShellToolDef {
            name: "stderr_test".into(),
            label: "Stderr Test".into(),
            description: "Writes to stderr".into(),
            readonly: true,
            params: HashMap::new(),
            exec: ShellExecDef {
                command: "sh".into(),
                args: vec!["-c".into(), "echo stdout_msg; echo stderr_msg >&2".into()],
                timeout: 5,
                truncate: "head".into(),
                install_hint: None,
            },
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let result = tool
            .execute("call-4", json!({}), test_ctx(temp_dir.path()))
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = match &result.content[0] {
            imp_llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        };
        assert!(text.contains("stdout_msg"));
        assert!(text.contains("stderr_msg"));
    }

    #[tokio::test]
    async fn shell_tool_timeout() {
        let tool = ShellTool::new(ShellToolDef {
            name: "slow".into(),
            label: "Slow".into(),
            description: "Times out".into(),
            readonly: true,
            params: HashMap::new(),
            exec: ShellExecDef {
                command: "sleep".into(),
                args: vec!["60".into()],
                timeout: 1,
                truncate: "head".into(),
                install_hint: None,
            },
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let result = tool
            .execute("call-5", json!({}), test_ctx(temp_dir.path()))
            .await
            .unwrap();

        assert!(result.is_error);
        assert_eq!(result.details["timed_out"], true);
    }

    #[tokio::test]
    async fn shell_tool_reports_missing_commands_with_install_hint() {
        let tool = ShellTool::new(ShellToolDef {
            name: "missing".into(),
            label: "Missing".into(),
            description: "Missing command".into(),
            readonly: true,
            params: HashMap::new(),
            exec: ShellExecDef {
                command: "definitely-not-a-real-command".into(),
                args: Vec::new(),
                timeout: 5,
                truncate: "head".into(),
                install_hint: Some("brew install definitely-not-a-real-command".into()),
            },
        });

        let temp_dir = tempfile::tempdir().unwrap();
        let result = tool
            .execute("call-2", json!({}), test_ctx(temp_dir.path()))
            .await
            .unwrap();

        assert!(result.is_error);
        let text = match &result.content[0] {
            imp_llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text output"),
        };
        assert!(text.contains("Command not found"));
        assert!(text.contains("Install hint"));
    }
}
