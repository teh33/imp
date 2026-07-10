use std::path::PathBuf;

use imp_core::config::PolicyAction;
use imp_core::tools::browser::{BrowserConfig, BrowserDiagnostic};

#[derive(Debug, Clone)]
pub(crate) enum BrowserHealth {
    NotChecked,
    Checking,
    Ready { version: String, binary: PathBuf },
    NotReady(String),
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserSettings {
    pub enabled: bool,
    pub binary: String,
    pub max_sessions: usize,
    pub timeout_ms: u64,
    pub idle_timeout_seconds: u64,
    pub max_response_bytes: usize,
    pub obey_robots: bool,
    pub block_private_networks: bool,
    pub input_policy: PolicyAction,
    pub health: BrowserHealth,
}

impl BrowserSettings {
    pub(super) fn new(config: &imp_core::config::Config) -> Self {
        Self {
            enabled: config.browser.enabled,
            binary: config
                .browser
                .binary
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            max_sessions: config.browser.max_sessions,
            timeout_ms: config.browser.timeout_ms,
            idle_timeout_seconds: config.browser.idle_timeout_seconds,
            max_response_bytes: config.browser.max_response_bytes,
            obey_robots: config.browser.obey_robots,
            block_private_networks: config.browser.block_private_networks,
            input_policy: config.policy.browser_input,
            health: BrowserHealth::NotChecked,
        }
    }

    pub(crate) fn config(&self) -> BrowserConfig {
        BrowserConfig {
            enabled: self.enabled,
            binary: (!self.binary.trim().is_empty()).then(|| PathBuf::from(self.binary.trim())),
            max_sessions: self.max_sessions,
            timeout_ms: self.timeout_ms,
            idle_timeout_seconds: self.idle_timeout_seconds,
            max_response_bytes: self.max_response_bytes,
            obey_robots: self.obey_robots,
            block_private_networks: self.block_private_networks,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        self.config().validate()
    }

    pub(super) fn apply(&self, config: &mut imp_core::config::Config) {
        config.browser = self.config();
        config.policy.browser_input = self.input_policy;
    }

    pub(crate) fn apply_diagnostic(&mut self, report: BrowserDiagnostic) {
        self.health = if report.ready {
            BrowserHealth::Ready {
                version: report.version.unwrap_or_else(|| "unknown".into()),
                binary: report.binary.unwrap_or_else(|| PathBuf::from("lightpanda")),
            }
        } else {
            let reason = report
                .checks
                .iter()
                .rev()
                .find(|check| check.status == imp_core::tools::browser::DiagnosticStatus::Fail)
                .map(|check| check.message.clone())
                .unwrap_or_else(|| "browser is disabled or unavailable".into());
            BrowserHealth::NotReady(reason)
        };
    }

    pub(super) fn health_label(&self) -> String {
        match &self.health {
            BrowserHealth::NotChecked => "not checked".into(),
            BrowserHealth::Checking => "checking…".into(),
            BrowserHealth::Ready { version, binary } => {
                format!("ready · {version} · {}", binary.display())
            }
            BrowserHealth::NotReady(reason) => format!("not ready · {reason}"),
        }
    }

    pub(super) fn invalidate_health(&mut self) {
        self.health = BrowserHealth::NotChecked;
    }

    pub(super) fn cycle_policy(&mut self, forward: bool) {
        self.input_policy = match (self.input_policy, forward) {
            (PolicyAction::Allow, true) | (PolicyAction::Deny, false) => PolicyAction::Ask,
            (PolicyAction::Ask, true) | (PolicyAction::Allow, false) => PolicyAction::Deny,
            (PolicyAction::Deny, true) | (PolicyAction::Ask, false) => PolicyAction::Allow,
        };
    }

    pub(super) fn policy_label(&self) -> &'static str {
        match self.input_policy {
            PolicyAction::Allow => "allow",
            PolicyAction::Ask => "ask",
            PolicyAction::Deny => "deny",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_settings_round_trip_and_validate() {
        let mut config = imp_core::config::Config::default();
        let mut settings = BrowserSettings::new(&config);
        settings.enabled = false;
        settings.binary = "/opt/lightpanda".into();
        settings.max_sessions = 4;
        settings.input_policy = PolicyAction::Allow;
        assert!(settings.validate().is_ok());
        settings.apply(&mut config);
        assert!(!config.browser.enabled);
        assert_eq!(
            config.browser.binary,
            Some(PathBuf::from("/opt/lightpanda"))
        );
        assert_eq!(config.browser.max_sessions, 4);
        assert_eq!(config.policy.browser_input, PolicyAction::Allow);
    }

    #[test]
    fn browser_settings_reject_invalid_session_count() {
        let config = imp_core::config::Config::default();
        let mut settings = BrowserSettings::new(&config);
        settings.max_sessions = 0;
        assert!(settings.validate().unwrap_err().contains("max_sessions"));
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use imp_core::tools::browser::{DiagnosticCheck, DiagnosticStatus};

    #[test]
    fn ready_diagnostic_is_rendered_with_version_and_binary() {
        let config = imp_core::config::Config::default();
        let mut settings = BrowserSettings::new(&config);
        settings.apply_diagnostic(BrowserDiagnostic {
            enabled: true,
            ready: true,
            binary: Some(PathBuf::from("/opt/lightpanda")),
            version: Some("0.3.4".into()),
            version_supported: true,
            protocol_compatible: true,
            missing_tools: Vec::new(),
            checks: vec![DiagnosticCheck {
                name: "mcp",
                status: DiagnosticStatus::Pass,
                message: "ready".into(),
            }],
        });
        assert_eq!(settings.health_label(), "ready · 0.3.4 · /opt/lightpanda");
    }

    #[test]
    fn failed_diagnostic_uses_last_failure_reason() {
        let config = imp_core::config::Config::default();
        let mut settings = BrowserSettings::new(&config);
        settings.apply_diagnostic(BrowserDiagnostic {
            enabled: true,
            ready: false,
            binary: None,
            version: None,
            version_supported: false,
            protocol_compatible: false,
            missing_tools: Vec::new(),
            checks: vec![DiagnosticCheck {
                name: "binary",
                status: DiagnosticStatus::Fail,
                message: "Lightpanda was not found".into(),
            }],
        });
        assert_eq!(
            settings.health_label(),
            "not ready · Lightpanda was not found"
        );
    }
}
