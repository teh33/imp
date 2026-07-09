use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::super::BrowserConfig;

const PROTOCOL_VERSION: &str = "2024-11-05";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

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
    let mut child = mcp_command(binary, config)
        .spawn()
        .map_err(|error| format!("could not start Lightpanda MCP: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("Lightpanda stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("Lightpanda stdout unavailable")?;
    stdin
        .write_all(probe_request().as_bytes())
        .await
        .map_err(|error| format!("could not write Lightpanda MCP probe: {error}"))?;
    stdin.flush().await.map_err(|error| error.to_string())?;
    let result = read_probe(BufReader::new(stdout)).await;
    let _ = child.kill().await;
    result
}

fn mcp_command(binary: &Path, config: &BrowserConfig) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("mcp")
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
    command
}

fn probe_request() -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"{PROTOCOL_VERSION}\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"imp-doctor\",\"version\":\"{}\"}}}}}}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{}}}}\n",
        env!("CARGO_PKG_VERSION")
    )
}

async fn read_probe(stdout: BufReader<tokio::process::ChildStdout>) -> Result<Vec<String>, String> {
    tokio::time::timeout(PROBE_TIMEOUT, async {
        let mut lines = stdout.lines();
        let initialize = next_line(&mut lines, "MCP initialization").await?;
        let initialized: serde_json::Value = serde_json::from_str(&initialize)
            .map_err(|error| format!("invalid MCP initialize response: {error}"))?;
        if initialized.get("error").is_some() {
            return Err("Lightpanda rejected MCP initialization".to_string());
        }
        let listed = next_line(&mut lines, "tools/list").await?;
        parse_tools(&listed)
    })
    .await
    .map_err(|_| "Lightpanda MCP probe timed out".to_string())?
}

async fn next_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    operation: &str,
) -> Result<String, String> {
    lines
        .next_line()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Lightpanda exited during {operation}"))
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
