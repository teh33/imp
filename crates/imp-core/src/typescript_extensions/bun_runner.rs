use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use imp_llm::ContentBlock;

use crate::child_process::{isolate_std_command, kill_std_process_group};
use crate::error::{Error, Result};

use super::discovery::TypeScriptExtension;
use super::pi_compat::BUN_BRIDGE;
use super::schema::{
    TypeScriptExtensionManifest, TypeScriptExtensionProtocol, TypeScriptExtensionToolManifest,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TypeScriptExtensionCallRequest<'a> {
    pub extension: &'a TypeScriptExtension,
    pub manifest: &'a TypeScriptExtensionManifest,
    pub tool: &'a TypeScriptExtensionToolManifest,
    pub arguments: serde_json::Value,
    pub env: Vec<(String, String)>,
    pub cwd: std::path::PathBuf,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct TypeScriptExtensionCallResult {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeScriptExtensionProtocolRequest<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    tool: &'a str,
    input: &'a serde_json::Value,
    context: TypeScriptExtensionProtocolContext<'a>,
    env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeScriptExtensionProtocolContext<'a> {
    cwd: String,
    extension_id: &'a str,
    run_id: Option<&'a str>,
    timeout_ms: u64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypeScriptExtensionProtocolResponse {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    details: Option<serde_json::Value>,
    #[serde(default)]
    is_error: bool,
}

pub(super) fn execute_manifest_tool(
    request: TypeScriptExtensionCallRequest<'_>,
) -> Result<TypeScriptExtensionCallResult> {
    match request.manifest.runtime.protocol {
        TypeScriptExtensionProtocol::OneShotJson => execute_one_shot_json(request),
        TypeScriptExtensionProtocol::JsonLines => execute_one_shot_json(request),
    }
}

fn execute_one_shot_json(
    request: TypeScriptExtensionCallRequest<'_>,
) -> Result<TypeScriptExtensionCallResult> {
    let timeout_ms = request
        .tool
        .timeout_ms
        .min(request.manifest.runtime.timeout_ms);
    let protocol_request = TypeScriptExtensionProtocolRequest {
        id: "call-1",
        kind: "tool_call",
        tool: &request.tool.name,
        input: &request.arguments,
        context: TypeScriptExtensionProtocolContext {
            cwd: request.cwd.display().to_string(),
            extension_id: &request.manifest.id,
            run_id: request.run_id.as_deref(),
            timeout_ms,
        },
        env: request.env.iter().cloned().collect(),
    };
    let request_json = serde_json::to_vec(&protocol_request)?;
    let stdout = run_manifest_command(
        request.extension,
        request.manifest,
        request.tool,
        &request.env,
        &request_json,
        timeout_ms,
    )?;
    let response: TypeScriptExtensionProtocolResponse =
        serde_json::from_slice(&stdout).map_err(|err| {
            Error::Tool(format!(
                "TypeScript extension '{}' returned invalid JSON: {err}",
                request.manifest.id
            ))
        })?;
    normalize_protocol_response(request.manifest, request.tool, response)
}

fn run_manifest_command(
    extension: &TypeScriptExtension,
    manifest: &TypeScriptExtensionManifest,
    tool: &TypeScriptExtensionToolManifest,
    env: &[(String, String)],
    stdin: &[u8],
    timeout_ms: u64,
) -> Result<Vec<u8>> {
    let mut command = Command::new(&manifest.runtime.command);
    command
        .args(&manifest.runtime.args)
        .current_dir(&extension.root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("IMP_EXTENSION_ID", &manifest.id)
        .env("IMP_EXTENSION_TOOL", &tool.name)
        .env("IMP_EXTENSION_ROOT", extension.root.display().to_string())
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    isolate_std_command(&mut command);

    let mut child = command.spawn().map_err(|err| {
        Error::Tool(format!(
            "failed to start TypeScript extension '{}': {err}",
            manifest.id
        ))
    })?;

    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin.write_all(stdin).map_err(|err| {
            Error::Tool(format!(
                "failed to write request to TypeScript extension '{}': {err}",
                manifest.id
            ))
        })?;
    }

    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_ms.max(1));
    loop {
        if let Some(status) = child.try_wait().map_err(Error::from)? {
            let output = child.wait_with_output().map_err(Error::from)?;
            if !status.success() {
                return Err(Error::Tool(format!(
                    "TypeScript extension '{}' failed: {}",
                    manifest.id,
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            if output.stdout.len() as u64 > manifest.runtime.output_limit_bytes {
                return Err(Error::Tool(format!(
                    "TypeScript extension '{}' exceeded output limit of {} bytes",
                    manifest.id, manifest.runtime.output_limit_bytes
                )));
            }
            return Ok(output.stdout);
        }
        if started.elapsed() >= timeout {
            kill_std_process_group(&child);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Tool(format!(
                "TypeScript extension '{}' timed out after {}ms",
                manifest.id, timeout_ms
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn normalize_protocol_response(
    manifest: &TypeScriptExtensionManifest,
    tool: &TypeScriptExtensionToolManifest,
    response: TypeScriptExtensionProtocolResponse,
) -> Result<TypeScriptExtensionCallResult> {
    if response.id.as_deref().is_some_and(|id| id != "call-1") {
        return Err(Error::Tool(format!(
            "TypeScript extension '{}' returned mismatched response id for tool '{}'",
            manifest.id, tool.name
        )));
    }

    match response.kind.as_str() {
        "tool_result" | "result" => Ok(TypeScriptExtensionCallResult {
            content: response
                .content
                .map(content_blocks_from_json)
                .transpose()?
                .unwrap_or_default(),
            is_error: response.is_error,
            details: response.details,
        }),
        "tool_error" | "error" => Ok(TypeScriptExtensionCallResult {
            content: vec![ContentBlock::Text {
                text: response
                    .message
                    .unwrap_or_else(|| "TypeScript extension tool error".into()),
            }],
            is_error: true,
            details: Some(serde_json::json!({ "code": response.code })),
        }),
        other => Err(Error::Tool(format!(
            "TypeScript extension '{}' returned unsupported response type '{}' for tool '{}'",
            manifest.id, other, tool.name
        ))),
    }
}

fn content_blocks_from_json(value: serde_json::Value) -> Result<Vec<ContentBlock>> {
    let blocks = value
        .as_array()
        .ok_or_else(|| Error::Tool("TypeScript extension content must be an array".into()))?;
    let mut content = Vec::new();
    for block in blocks {
        let kind = block
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");
        match kind {
            "text" => content.push(ContentBlock::Text {
                text: block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            other => {
                return Err(Error::Tool(format!(
                    "TypeScript extension returned unsupported content block type '{other}'"
                )));
            }
        }
    }
    Ok(content)
}

pub(super) fn run_bun_bridge(
    extension: &TypeScriptExtension,
    action: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value> {
    ensure_bun_available()?;

    let bridge = std::env::temp_dir().join("imp-ts-extension-bridge.ts");
    std::fs::write(&bridge, BUN_BRIDGE).map_err(Error::from)?;

    let mut command = Command::new("bun");
    command
        .arg(&bridge)
        .arg(action)
        .arg(&extension.entrypoint)
        .arg(serde_json::to_string(&payload)?)
        .current_dir(&extension.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    isolate_std_command(&mut command);

    let output = run_bun_bridge_command(command)?;

    if !output.status.success() {
        return Err(Error::Tool(format!(
            "TypeScript extension '{}' failed: {}",
            extension.name,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(Error::from)
}

fn run_bun_bridge_command(mut command: Command) -> Result<std::process::Output> {
    let mut child = command.spawn().map_err(Error::from)?;
    let started = Instant::now();
    loop {
        if let Some(_status) = child.try_wait().map_err(Error::from)? {
            return child.wait_with_output().map_err(Error::from);
        }
        if started.elapsed() >= BUN_BRIDGE_TIMEOUT {
            kill_std_process_group(&child);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Tool(format!(
                "TypeScript extension bridge timed out after {}s",
                BUN_BRIDGE_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn ensure_bun_available() -> Result<()> {
    Command::new("bun")
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|_| {
            Error::Tool(
                "TypeScript extensions require Bun. Install Bun and ensure `bun` is on PATH."
                    .to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typescript_extensions::schema::{
        TypeScriptExtensionNetwork, TypeScriptExtensionProtocol, TypeScriptExtensionResourceScope,
        TypeScriptExtensionRuntimeKind, TypeScriptExtensionRuntimeManifest,
        TypeScriptExtensionSideEffect, TypeScriptExtensionToolManifest,
        TYPESCRIPT_EXTENSION_MANIFEST_SCHEMA_VERSION,
    };
    use crate::typescript_extensions::{TypeScriptExtension, TypeScriptExtensionSource};
    use tempfile::TempDir;

    fn manifest(command: String, args: Vec<String>) -> TypeScriptExtensionManifest {
        TypeScriptExtensionManifest {
            schema_version: TYPESCRIPT_EXTENSION_MANIFEST_SCHEMA_VERSION,
            id: "example.echo".into(),
            name: "Example Echo".into(),
            version: "0.1.0".into(),
            runtime: TypeScriptExtensionRuntimeManifest {
                kind: TypeScriptExtensionRuntimeKind::TypeScriptSubprocess,
                command,
                args,
                protocol: TypeScriptExtensionProtocol::OneShotJson,
                timeout_ms: 2_000,
                output_limit_bytes: 65_536,
            },
            tools: vec![tool()],
        }
    }

    fn tool() -> TypeScriptExtensionToolManifest {
        TypeScriptExtensionToolManifest {
            name: "example_echo".into(),
            label: Some("Echo".into()),
            description: "Echo text".into(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            side_effect: TypeScriptExtensionSideEffect::ReadOnly,
            resource_scope: TypeScriptExtensionResourceScope::None,
            network: TypeScriptExtensionNetwork::None,
            secrets: Vec::new(),
            env: Vec::new(),
            timeout_ms: 2_000,
            output_limit_bytes: 65_536,
            policy_tags: Vec::new(),
            verifier_tags: Vec::new(),
        }
    }

    fn extension(root: &std::path::Path) -> TypeScriptExtension {
        TypeScriptExtension {
            name: "example".into(),
            root: root.to_path_buf(),
            entrypoint: root.join("tool.py"),
            source: TypeScriptExtensionSource::Local,
            manifest_path: None,
        }
    }

    fn write_python_tool(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let tool = root.join("tool.py");
        std::fs::write(&tool, body).unwrap();
        tool
    }

    fn assert_text_content(content: &[ContentBlock], expected: &str) {
        let Some(ContentBlock::Text { text }) = content.first() else {
            panic!("expected text content block, got {content:?}");
        };
        assert_eq!(text, expected);
    }

    #[test]
    fn execute_manifest_tool_sends_request_and_reads_text_result() {
        let temp = TempDir::new().unwrap();
        let script = write_python_tool(
            temp.path(),
            r#"import json, sys
request = json.load(sys.stdin)
assert request["type"] == "tool_call"
assert request["tool"] == "example_echo"
print(json.dumps({
    "id": request["id"],
    "type": "tool_result",
    "content": [{"type": "text", "text": request["input"]["text"]}],
    "details": {"ok": True}
}))
"#,
        );
        let manifest = manifest("python3".into(), vec![script.display().to_string()]);
        let ext = extension(temp.path());
        let tool = manifest.tools[0].clone();

        let result = execute_manifest_tool(TypeScriptExtensionCallRequest {
            extension: &ext,
            manifest: &manifest,
            tool: &tool,
            arguments: serde_json::json!({ "text": "hello" }),
            env: Vec::new(),
            cwd: temp.path().to_path_buf(),
            run_id: Some("run-1".into()),
        })
        .unwrap();

        assert!(!result.is_error);
        assert_text_content(&result.content, "hello");
        assert_eq!(result.details.unwrap()["ok"], true);
    }

    #[test]
    fn execute_manifest_tool_converts_error_response_to_tool_error() {
        let temp = TempDir::new().unwrap();
        let script = write_python_tool(
            temp.path(),
            r#"import json
print(json.dumps({"id":"call-1","type":"error","message":"bad input","code":"invalid"}))
"#,
        );
        let manifest = manifest("python3".into(), vec![script.display().to_string()]);
        let ext = extension(temp.path());
        let tool = manifest.tools[0].clone();

        let result = execute_manifest_tool(TypeScriptExtensionCallRequest {
            extension: &ext,
            manifest: &manifest,
            tool: &tool,
            arguments: serde_json::json!({}),
            env: Vec::new(),
            cwd: temp.path().to_path_buf(),
            run_id: None,
        })
        .unwrap();

        assert!(result.is_error);
        assert_text_content(&result.content, "bad input");
        assert_eq!(result.details.unwrap()["code"], "invalid");
    }

    #[test]
    fn execute_manifest_tool_rejects_invalid_json() {
        let temp = TempDir::new().unwrap();
        let script = write_python_tool(temp.path(), "print('not json')\n");
        let manifest = manifest("python3".into(), vec![script.display().to_string()]);
        let ext = extension(temp.path());
        let tool = manifest.tools[0].clone();

        let err = execute_manifest_tool(TypeScriptExtensionCallRequest {
            extension: &ext,
            manifest: &manifest,
            tool: &tool,
            arguments: serde_json::json!({}),
            env: Vec::new(),
            cwd: temp.path().to_path_buf(),
            run_id: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("returned invalid JSON"));
    }

    #[test]
    fn execute_manifest_tool_passes_only_mediated_env() {
        let temp = TempDir::new().unwrap();
        let script = write_python_tool(
            temp.path(),
            r#"import json, os, sys
json.load(sys.stdin)
print(json.dumps({
    "id":"call-1",
    "type":"tool_result",
    "content":[{"type":"text","text": os.environ.get("ALLOWED_VALUE", "missing") }],
    "details":{"hasBlocked": "BLOCKED_VALUE" in os.environ}
}))
"#,
        );
        let manifest = manifest("python3".into(), vec![script.display().to_string()]);
        let ext = extension(temp.path());
        let tool = manifest.tools[0].clone();

        let result = execute_manifest_tool(TypeScriptExtensionCallRequest {
            extension: &ext,
            manifest: &manifest,
            tool: &tool,
            arguments: serde_json::json!({}),
            env: vec![("ALLOWED_VALUE".into(), "visible".into())],
            cwd: temp.path().to_path_buf(),
            run_id: None,
        })
        .unwrap();

        assert_text_content(&result.content, "visible");
        assert_eq!(result.details.unwrap()["hasBlocked"], false);
    }

    #[test]
    fn execute_manifest_tool_enforces_timeout() {
        let temp = TempDir::new().unwrap();
        let script = write_python_tool(temp.path(), "import time\ntime.sleep(1)\n");
        let mut manifest = manifest("python3".into(), vec![script.display().to_string()]);
        manifest.runtime.timeout_ms = 25;
        manifest.tools[0].timeout_ms = 25;
        let ext = extension(temp.path());
        let tool = manifest.tools[0].clone();

        let err = execute_manifest_tool(TypeScriptExtensionCallRequest {
            extension: &ext,
            manifest: &manifest,
            tool: &tool,
            arguments: serde_json::json!({}),
            env: Vec::new(),
            cwd: temp.path().to_path_buf(),
            run_id: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }
}
