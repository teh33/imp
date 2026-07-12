use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessId, ProcessManager,
    ProcessMode, ProcessRequest,
};

use super::super::BrowserConfig;

const PROTOCOL_VERSION: &str = "2024-11-05";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_RETENTION_BYTES: usize = 64 * 1024;

pub(super) const REQUIRED_TOOLS: &[&str] = &[
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

pub(super) async fn probe_mcp(
    binary: &Path,
    config: &BrowserConfig,
) -> Result<Vec<String>, String> {
    probe_mcp_with_timeout(binary, config, PROBE_TIMEOUT).await
}

async fn probe_mcp_with_timeout(
    binary: &Path,
    config: &BrowserConfig,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let manager = ProcessManager::new();
    let process = manager
        .start(probe_request_spec(binary, config, timeout)?)
        .await
        .map_err(|error| format!("could not start Lightpanda MCP: {error}"))?;
    manager
        .write(process.id, probe_request().as_bytes())
        .await
        .map_err(|error| format!("could not write Lightpanda MCP probe: {error}"))?;
    let result = read_probe(&manager, process.id, timeout).await;
    let _ = manager.cancel(process.id).await;
    result
}

fn probe_request_spec(
    binary: &Path,
    config: &BrowserConfig,
    timeout: Duration,
) -> Result<ProcessRequest, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let environment = BTreeMap::from([
        ("LIGHTPANDA_DISABLE_TELEMETRY".into(), "true".into()),
        ("LIGHTPANDA_DISABLE_CORE_DUMP".into(), "1".into()),
    ]);
    let mut grant = ExecutionGrant::host(&cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let mut arguments = vec!["mcp".into()];
    if config.obey_robots {
        arguments.push("--obey-robots".into());
    }
    if config.block_private_networks {
        arguments.push("--block-private-networks".into());
    }
    Ok(ProcessRequest {
        command: CommandSpec::new(binary.display().to_string()).with_arguments(arguments),
        cwd,
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(timeout),
        output_retention_bytes: OUTPUT_RETENTION_BYTES,
        grant,
    })
}

fn probe_request() -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"{PROTOCOL_VERSION}\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"imp-doctor\",\"version\":\"{}\"}}}}}}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{}}}}\n",
        env!("CARGO_PKG_VERSION")
    )
}

async fn read_probe(
    manager: &ProcessManager,
    id: ProcessId,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut cursor = OutputCursor::start(id);
    let mut lines = LineBuffer::default();
    let initialize = next_response(
        manager,
        id,
        &mut cursor,
        &mut lines,
        deadline,
        "MCP initialization",
    )
    .await?;
    let initialized: serde_json::Value = serde_json::from_str(&initialize)
        .map_err(|error| format!("invalid MCP initialize response: {error}"))?;
    if initialized.get("error").is_some() {
        return Err("Lightpanda rejected MCP initialization".into());
    }
    let listed =
        next_response(manager, id, &mut cursor, &mut lines, deadline, "tools/list").await?;
    parse_tools(&listed)
}

async fn next_response(
    manager: &ProcessManager,
    id: ProcessId,
    cursor: &mut OutputCursor,
    lines: &mut LineBuffer,
    deadline: tokio::time::Instant,
    operation: &str,
) -> Result<String, String> {
    loop {
        if let Some(line) = lines.pop() {
            return Ok(line);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("Lightpanda MCP probe timed out".into());
        }
        let output = manager
            .observe(id, *cursor, deadline - now, usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
        *cursor = output.next_cursor;
        for chunk in output.chunks {
            if chunk.stream == OutputStream::Stdout {
                lines.push(&chunk.text);
            }
        }
        if output.state.is_terminal() {
            lines.finish();
            if let Some(line) = lines.pop() {
                return Ok(line);
            }
            let exit = manager.wait(id).await.map_err(|error| error.to_string())?;
            return if exit.timed_out {
                Err("Lightpanda MCP probe timed out".into())
            } else {
                Err(format!("Lightpanda exited during {operation}"))
            };
        }
    }
}

#[derive(Default)]
struct LineBuffer {
    pending: String,
    ready: VecDeque<String>,
}

impl LineBuffer {
    fn push(&mut self, text: &str) {
        self.pending.push_str(text);
        while let Some(index) = self.pending.find('\n') {
            let line = self.pending[..index].trim_end_matches('\r').to_string();
            self.pending.drain(..=index);
            self.ready.push_back(line);
        }
    }

    fn pop(&mut self) -> Option<String> {
        self.ready.pop_front()
    }

    fn finish(&mut self) {
        if !self.pending.is_empty() {
            self.ready
                .push_back(self.pending.trim_end_matches('\r').to_string());
            self.pending.clear();
        }
    }
}

fn parse_tools(response: &str) -> Result<Vec<String>, String> {
    let response: serde_json::Value = serde_json::from_str(response)
        .map_err(|error| format!("invalid MCP tools/list response: {error}"))?;
    let tools = response["result"]["tools"]
        .as_array()
        .ok_or("Lightpanda tools/list response was invalid")?;
    Ok(tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect())
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
