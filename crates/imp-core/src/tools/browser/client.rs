use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::config::BrowserConfig;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub(crate) struct LightpandaClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    max_response_bytes: usize,
}

impl LightpandaClient {
    pub(crate) async fn spawn(
        config: &BrowserConfig,
        cwd: &std::path::Path,
    ) -> Result<Self, String> {
        let binary = super::resolve_lightpanda_binary(config.binary.as_deref())?;
        let mut command = Command::new(&binary);
        command
            .arg("mcp")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .env_clear()
            .env("LIGHTPANDA_DISABLE_TELEMETRY", "true")
            .env("LIGHTPANDA_DISABLE_CORE_DUMP", "1");
        if config.obey_robots {
            command.arg("--obey-robots");
        }
        if config.block_private_networks {
            command.arg("--block-private-networks");
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "could not start Lightpanda at {}: {error}. Install Lightpanda or set browser.binary",
                binary.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Lightpanda stdin was not available".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Lightpanda stdout was not available".to_string())?;
        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            max_response_bytes: config.max_response_bytes,
        };
        client
            .initialize(Duration::from_millis(config.timeout_ms))
            .await?;
        Ok(client)
    }

    pub(crate) async fn call(
        &mut self,
        tool: &str,
        arguments: Value,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<McpOutput, String> {
        let response = self
            .request_with_cancellation(
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
                timeout,
                cancelled,
            )
            .await?;
        parse_tool_result(response)
    }

    async fn initialize(&mut self, timeout: Duration) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "imp", "version": env!("CARGO_PKG_VERSION")}
            }),
            timeout,
        )
        .await?;
        self.write_message(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await?;
        self.validate_tools(timeout).await
    }

    async fn validate_tools(&mut self, timeout: Duration) -> Result<(), String> {
        let result = self.request("tools/list", json!({}), timeout).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| "Lightpanda tools/list response was invalid".to_string())?;
        for required in REQUIRED_TOOLS {
            let found = tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(*required));
            if !found {
                return Err(format!(
                    "Lightpanda is incompatible: required MCP tool `{required}` is missing"
                ));
            }
        }
        Ok(())
    }

    async fn request_with_cancellation(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Value, String> {
        tokio::select! {
            result = self.request(method, params, timeout) => result,
            _ = wait_for_cancellation(cancelled) => Err("browser action cancelled".into()),
        }
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_message(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;

        let line = tokio::time::timeout(
            timeout,
            read_line_bounded(&mut self.stdout, self.max_response_bytes),
        )
        .await
        .map_err(|_| {
            format!(
                "Lightpanda {method} timed out after {} ms",
                timeout.as_millis()
            )
        })??;
        let response: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("invalid JSON from Lightpanda: {error}"))?;
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("Lightpanda returned a mismatched response id".into());
        }
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown protocol error");
            return Err(format!("Lightpanda protocol error: {message}"));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "Lightpanda response did not include a result".into())
    }

    async fn write_message(&mut self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message)
            .map_err(|error| format!("failed encoding Lightpanda request: {error}"))?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .map_err(|error| format!("failed writing Lightpanda request: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("failed flushing Lightpanda request: {error}"))
    }

    pub(crate) fn abort(&mut self) {
        let _ = self.child.start_kill();
    }

    pub(crate) async fn shutdown(mut self) -> Result<(), String> {
        let _ = self.stdin.shutdown().await;
        match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(format!("failed waiting for Lightpanda: {error}")),
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .map_err(|error| format!("failed to stop Lightpanda: {error}"))?;
                Ok(())
            }
        }
    }
}

const REQUIRED_TOOLS: &[&str] = &[
    "goto",
    "interactiveElements",
    "markdown",
    "links",
    "structuredData",
    "detectForms",
    "extract",
    "click",
    "fill",
    "press",
    "selectOption",
    "setChecked",
    "scroll",
    "waitForSelector",
    "getUrl",
    "consoleLogs",
];

async fn wait_for_cancellation(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn read_line_bounded(
    reader: &mut BufReader<ChildStdout>,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|error| format!("failed reading Lightpanda response: {error}"))?;
        if available.is_empty() {
            return Err("Lightpanda exited before responding".into());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > max_bytes {
            return Err(format!("Lightpanda response exceeded {max_bytes} bytes"));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            return Ok(line);
        }
    }
}

#[derive(Debug)]
pub(crate) struct McpOutput {
    pub(crate) text: String,
    pub(crate) is_error: bool,
}

fn parse_tool_result(result: Value) -> Result<McpOutput, String> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| "Lightpanda tool result did not include content".to_string())?;
    let text = content
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(McpOutput {
        text,
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

impl Drop for LightpandaClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_tool_result() {
        let result = parse_tool_result(json!({
            "content": [{"type": "text", "text": "page state"}],
            "isError": false
        }))
        .unwrap();
        assert_eq!(result.text, "page state");
        assert!(!result.is_error);
    }

    #[test]
    fn rejects_tool_result_without_content() {
        let error = parse_tool_result(json!({})).unwrap_err();
        assert!(error.contains("content"));
    }
}
