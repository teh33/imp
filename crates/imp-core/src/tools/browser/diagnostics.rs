use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

mod protocol;
mod version;
use protocol::{probe_mcp, REQUIRED_TOOLS};
use version::read_version;

use super::BrowserConfig;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_SUPPORTED_VERSION: (u64, u64, u64) = (0, 3, 4);

#[derive(Debug, Clone, Serialize)]
pub struct BrowserDiagnostic {
    pub enabled: bool,
    pub ready: bool,
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub version_supported: bool,
    pub protocol_compatible: bool,
    pub missing_tools: Vec<String>,
    pub checks: Vec<DiagnosticCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticCheck {
    pub name: &'static str,
    pub status: DiagnosticStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

pub async fn diagnose_browser(config: &BrowserConfig) -> BrowserDiagnostic {
    let mut report = BrowserDiagnostic {
        enabled: config.enabled,
        ready: false,
        binary: None,
        version: None,
        version_supported: false,
        protocol_compatible: false,
        missing_tools: Vec::new(),
        checks: Vec::new(),
    };
    if !config.enabled {
        report.checks.push(check(
            "configuration",
            DiagnosticStatus::Warn,
            "browser tool is disabled in configuration",
        ));
        return report;
    }
    match config.validate() {
        Ok(()) => report.checks.push(check(
            "configuration",
            DiagnosticStatus::Pass,
            "browser configuration is valid",
        )),
        Err(error) => {
            report
                .checks
                .push(check("configuration", DiagnosticStatus::Fail, error));
            return report;
        }
    }
    let binary = match resolve_lightpanda_binary(config.binary.as_deref()) {
        Ok(path) => path,
        Err(error) => {
            report
                .checks
                .push(check("binary", DiagnosticStatus::Fail, error));
            return report;
        }
    };
    report.binary = Some(binary.clone());
    report.checks.push(check(
        "binary",
        DiagnosticStatus::Pass,
        format!("found {}", binary.display()),
    ));
    match read_version(&binary, PROBE_TIMEOUT).await {
        Ok(version) => {
            report.version_supported = version_is_supported(&version);
            report.version = Some(version.clone());
            let status = if report.version_supported {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            };
            let message = if report.version_supported {
                format!("Lightpanda {version} is supported")
            } else {
                format!("Lightpanda {version} is older than required 0.3.4")
            };
            report.checks.push(check("version", status, message));
        }
        Err(error) => {
            report
                .checks
                .push(check("version", DiagnosticStatus::Fail, error));
            return report;
        }
    }
    match probe_mcp(&binary, config).await {
        Ok(tools) => {
            report.protocol_compatible = true;
            report.missing_tools = REQUIRED_TOOLS
                .iter()
                .filter(|required| !tools.iter().any(|tool| tool == **required))
                .map(|tool| (*tool).to_string())
                .collect();
            let status = if report.missing_tools.is_empty() {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            };
            let message = if report.missing_tools.is_empty() {
                format!(
                    "MCP initialized with all {} required tools",
                    REQUIRED_TOOLS.len()
                )
            } else {
                format!("missing MCP tools: {}", report.missing_tools.join(", "))
            };
            report.checks.push(check("mcp", status, message));
        }
        Err(error) => report
            .checks
            .push(check("mcp", DiagnosticStatus::Fail, error)),
    }
    report.ready =
        report.version_supported && report.protocol_compatible && report.missing_tools.is_empty();
    report
}

pub fn resolve_lightpanda_binary(configured: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return executable_path(path).ok_or_else(|| {
            format!(
                "configured Lightpanda binary is not executable: {}. Install Lightpanda or update browser.binary",
                path.display()
            )
        });
    }
    let Some(path) = std::env::var_os("PATH") else {
        return Err("PATH is not set and browser.binary is not configured".into());
    };
    for directory in std::env::split_paths(&path) {
        if let Some(path) = executable_path(&directory.join(binary_name())) {
            return Ok(path);
        }
    }
    Err("Lightpanda was not found. Install it or set browser.binary".into())
}

fn version_is_supported(version: &str) -> bool {
    parse_version(version).is_some_and(|version| version >= MIN_SUPPORTED_VERSION)
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let numeric = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = numeric.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn check(
    name: &'static str,
    status: DiagnosticStatus,
    message: impl Into<String>,
) -> DiagnosticCheck {
    DiagnosticCheck {
        name,
        status,
        message: message.into(),
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "lightpanda.exe"
    } else {
        "lightpanda"
    }
}

fn executable_path(path: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return None;
    }
    std::fs::canonicalize(path)
        .ok()
        .or_else(|| Some(path.to_path_buf()))
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_versions() {
        assert!(version_is_supported("0.3.4"));
        assert!(version_is_supported("v0.4.0-beta"));
        assert!(!version_is_supported("0.3.3"));
        assert!(!version_is_supported("nightly"));
    }
}
