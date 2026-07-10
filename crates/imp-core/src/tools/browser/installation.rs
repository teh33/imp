use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserInstallMethod {
    Homebrew,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserInstallPlan {
    pub method: BrowserInstallMethod,
    pub program: String,
    pub arguments: Vec<String>,
}

impl BrowserInstallPlan {
    pub fn command_display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn browser_install_plan() -> Result<BrowserInstallPlan, String> {
    if cfg!(target_os = "macos") && command_on_path("brew") {
        return Ok(BrowserInstallPlan {
            method: BrowserInstallMethod::Homebrew,
            program: "brew".into(),
            arguments: vec!["install".into(), "lightpanda-io/browser/lightpanda".into()],
        });
    }
    Err(
        "no supported package manager found; install Lightpanda from https://lightpanda.io/docs/installation/"
            .into(),
    )
}

pub async fn install_browser(plan: &BrowserInstallPlan) -> Result<(), String> {
    let status = tokio::process::Command::new(&plan.program)
        .args(&plan.arguments)
        .status()
        .await
        .map_err(|error| format!("could not run {}: {error}", plan.program))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} installation failed with status {status}",
            install_method_label(plan.method)
        ))
    }
}

fn install_method_label(method: BrowserInstallMethod) -> &'static str {
    match method {
        BrowserInstallMethod::Homebrew => "Homebrew",
    }
}

fn command_on_path(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| executable(&directory.join(command)))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata)
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
    fn install_plan_display_is_auditable() {
        let plan = BrowserInstallPlan {
            method: BrowserInstallMethod::Homebrew,
            program: "brew".into(),
            arguments: vec!["install".into(), "lightpanda".into()],
        };
        assert_eq!(plan.command_display(), "brew install lightpanda");
    }
}
