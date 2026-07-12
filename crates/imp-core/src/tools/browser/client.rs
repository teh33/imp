use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::client_process::process_request;
use super::config::BrowserConfig;
use crate::process::{OutputCursor, OutputStream, ProcessId, ProcessManager, StopOptions};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub(crate) struct LightpandaClient {
    manager: ProcessManager,
    process_id: ProcessId,
    cursor: OutputCursor,
    response_buffer: String,
    next_id: u64,
    max_response_bytes: usize,
}

impl LightpandaClient {
    pub(crate) async fn spawn(
        config: &BrowserConfig,
        cwd: &std::path::Path,
    ) -> Result<Self, String> {
        let binary = super::resolve_lightpanda_binary(config.binary.as_deref())?;
        let manager = ProcessManager::new();
        let process = manager.start(process_request(&binary, config, cwd)).await.map_err(|error| {
            format!(
                "could not start Lightpanda at {}: {error}. Install Lightpanda or set browser.binary",
                binary.display()
            )
        })?;
        let mut client = Self {
            manager,
            process_id: process.id,
            cursor: OutputCursor::start(process.id),
            response_buffer: String::new(),
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
            if !tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(*required))
            {
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
        let line = self.read_response(method, timeout).await?;
        let response: Value = serde_json::from_str(&line)
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

    async fn read_response(&mut self, method: &str, timeout: Duration) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(line) = take_line(&mut self.response_buffer) {
                if line.len() > self.max_response_bytes {
                    return Err(response_limit_error(self.max_response_bytes));
                }
                return Ok(line);
            }
            if self.response_buffer.len() > self.max_response_bytes {
                return Err(response_limit_error(self.max_response_bytes));
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(timeout_error(method, timeout));
            }
            let output = self
                .manager
                .observe(self.process_id, self.cursor, deadline - now, 64 * 1024)
                .await
                .map_err(|error| format!("failed reading Lightpanda response: {error}"))?;
            self.cursor = output.next_cursor;
            if output.unread_output_evicted {
                return Err(
                    "Lightpanda response output was evicted before it could be read".into(),
                );
            }
            for chunk in output.chunks {
                if chunk.stream == OutputStream::Stdout {
                    self.response_buffer.push_str(&chunk.text);
                }
            }
            if output.state.is_terminal() {
                return Err("Lightpanda exited before responding".into());
            }
        }
    }

    async fn write_message(&self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message)
            .map_err(|error| format!("failed encoding Lightpanda request: {error}"))?;
        encoded.push(b'\n');
        self.manager
            .write(self.process_id, &encoded)
            .await
            .map(|_| ())
            .map_err(|error| format!("failed writing Lightpanda request: {error}"))
    }

    pub(crate) fn abort(&mut self) {
        self.manager = ProcessManager::new();
    }

    pub(crate) async fn shutdown(self) -> Result<(), String> {
        self.manager
            .close_stdin(self.process_id)
            .await
            .map_err(|error| format!("failed closing Lightpanda stdin: {error}"))?;
        match tokio::time::timeout(Duration::from_secs(2), self.manager.wait(self.process_id)).await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(format!("failed waiting for Lightpanda: {error}")),
            Err(_) => self
                .manager
                .stop(self.process_id, StopOptions::default())
                .await
                .map(|_| ())
                .map_err(|error| format!("failed to stop Lightpanda: {error}")),
        }
    }
}

fn take_line(buffer: &mut String) -> Option<String> {
    let newline = buffer.find('\n')?;
    let line = buffer[..newline].trim_end_matches('\r').to_string();
    buffer.drain(..=newline);
    Some(line)
}

fn response_limit_error(max_bytes: usize) -> String {
    format!("Lightpanda response exceeded {max_bytes} bytes")
}

fn timeout_error(method: &str, timeout: Duration) -> String {
    format!(
        "Lightpanda {method} timed out after {} ms",
        timeout.as_millis()
    )
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

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
