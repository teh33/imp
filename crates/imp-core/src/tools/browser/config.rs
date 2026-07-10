use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn default_max_sessions() -> usize {
    2
}

fn default_timeout_ms() -> u64 {
    30_000
}

fn default_idle_timeout_seconds() -> u64 {
    300
}

fn default_max_response_bytes() -> usize {
    1024 * 1024
}

/// Configuration for the Lightpanda-backed browser tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub binary: Option<PathBuf>,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_true")]
    pub obey_robots: bool,
    #[serde(default = "default_true")]
    pub block_private_networks: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            binary: None,
            max_sessions: default_max_sessions(),
            timeout_ms: default_timeout_ms(),
            idle_timeout_seconds: default_idle_timeout_seconds(),
            max_response_bytes: default_max_response_bytes(),
            obey_robots: true,
            block_private_networks: true,
        }
    }
}

impl BrowserConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_sessions == 0 || self.max_sessions > 16 {
            return Err("browser.max_sessions must be between 1 and 16".into());
        }
        if !(100..=120_000).contains(&self.timeout_ms) {
            return Err("browser.timeout_ms must be between 100 and 120000".into());
        }
        if !(4096..=16 * 1024 * 1024).contains(&self.max_response_bytes) {
            return Err("browser.max_response_bytes must be between 4096 and 16777216".into());
        }
        Ok(())
    }
}
