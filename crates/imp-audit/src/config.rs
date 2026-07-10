use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

use crate::error::{AuditError, Result};

/// Top-level `.imp/audit.toml` configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Default audit behavior.
    #[serde(default)]
    pub defaults: ConfigDefaults,
    /// Named profiles mapping profile ids to check lists.
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    /// Configured checks.
    #[serde(default)]
    pub checks: Vec<CheckConfig>,
    /// External tool requirements used by configured checks.
    #[serde(default)]
    pub requirements: Vec<ToolRequirement>,
}

impl AuditConfig {
    /// Parse audit configuration from TOML.
    pub fn from_toml_str(input: &str) -> Result<Self> {
        let config: Self =
            toml::from_str(input).map_err(|err| AuditError::ConfigParse(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Return a configured requirement by id.
    #[must_use]
    pub fn requirement(&self, id: &str) -> Option<&ToolRequirement> {
        self.requirements
            .iter()
            .find(|requirement| requirement.id == id)
    }

    /// Return checks selected by profile id.
    ///
    /// When the profile is not configured, an error is returned instead of
    /// silently falling back to all checks.
    pub fn checks_for_profile(&self, profile: &str) -> Result<Vec<&CheckConfig>> {
        let profile_config = self
            .profiles
            .get(profile)
            .ok_or_else(|| AuditError::UnknownItem(profile.to_string()))?;
        let checks_by_id: BTreeMap<&str, &CheckConfig> = self
            .checks
            .iter()
            .map(|check| (check.id.as_str(), check))
            .collect();

        profile_config
            .checks
            .iter()
            .map(|check_id| {
                checks_by_id
                    .get(check_id.as_str())
                    .copied()
                    .ok_or_else(|| AuditError::UnknownItem(check_id.clone()))
            })
            .collect()
    }

    /// Validate semantic constraints that TOML parsing cannot express.
    pub fn validate(&self) -> Result<()> {
        reject_duplicate_ids("check", self.checks.iter().map(|check| check.id.as_str()))?;
        reject_duplicate_ids(
            "requirement",
            self.requirements
                .iter()
                .map(|requirement| requirement.id.as_str()),
        )?;

        let check_ids: HashSet<&str> = self.checks.iter().map(|check| check.id.as_str()).collect();
        for (profile_id, profile) in &self.profiles {
            if profile_id.trim().is_empty() {
                return Err(AuditError::ConfigValidation(
                    "profile id must not be empty".into(),
                ));
            }
            for check_id in &profile.checks {
                if !check_ids.contains(check_id.as_str()) {
                    return Err(AuditError::ConfigValidation(format!(
                        "profile `{profile_id}` references unknown check `{check_id}`"
                    )));
                }
            }
        }

        let requirements: HashSet<&str> = self
            .requirements
            .iter()
            .map(|requirement| requirement.id.as_str())
            .collect();
        for check in &self.checks {
            for requirement in &check.requires {
                if !requirements.contains(requirement.as_str()) {
                    return Err(AuditError::ConfigValidation(format!(
                        "check `{}` references unknown requirement `{requirement}`",
                        check.id
                    )));
                }
            }
        }

        for requirement in &self.requirements {
            reject_duplicate_ids(
                "install recipe",
                requirement
                    .installers
                    .iter()
                    .map(|installer| installer.id.as_str()),
            )?;
            for installer in &requirement.installers {
                if installer.command.is_empty() {
                    return Err(AuditError::ConfigValidation(format!(
                        "installer `{}` for requirement `{}` must declare a command",
                        installer.id, requirement.id
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Default audit behavior shared by profiles and checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDefaults {
    /// Default scope, such as `changed` or `workspace`.
    #[serde(default = "default_scope")]
    pub scope: String,
    /// Default base ref for changed-file comparisons.
    #[serde(default = "default_base")]
    pub base: String,
    /// Default command timeout in seconds.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Default maximum findings to show in compact summaries.
    #[serde(default = "default_max_findings")]
    pub max_findings: usize,
}

impl Default for ConfigDefaults {
    fn default() -> Self {
        Self {
            scope: default_scope(),
            base: default_base(),
            timeout_seconds: default_timeout_seconds(),
            max_findings: default_max_findings(),
        }
    }
}

/// Named collection of checks for a specific audit use case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Human-readable description of the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Check ids included in this profile.
    #[serde(default)]
    pub checks: Vec<String>,
}

/// One configured audit check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckConfig {
    /// Stable check identifier.
    pub id: String,
    /// Check kind, such as `lint`, `test`, `security`, or `ai_quality`.
    pub kind: String,
    /// Shell command to run for command-backed checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Native engine name for native checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Parser adapter name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    /// Glob-like path selectors for this check.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Whether this check may be skipped when its requirement is missing.
    #[serde(default)]
    pub optional: bool,
    /// Requirement ids needed to run this check.
    #[serde(default)]
    pub requires: Vec<String>,
}

/// External tool requirement for one or more checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRequirement {
    /// Stable requirement identifier.
    pub id: String,
    /// Binary expected on `PATH`.
    pub binary: String,
    /// Optional command used to detect the installed version.
    #[serde(default)]
    pub version_command: Vec<String>,
    /// Declarative install recipes that users may opt into.
    #[serde(default)]
    pub installers: Vec<InstallRecipe>,
}

/// Declarative, user-approved installation option for a missing requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecipe {
    /// Stable recipe identifier.
    pub id: String,
    /// User-facing label for this install choice.
    pub label: String,
    /// Package manager or installer kind, such as `npm`, `brew`, or `cargo`.
    pub manager: String,
    /// Exact command tokens to execute after user approval.
    pub command: Vec<String>,
    /// Installation scope, such as `project`, `user`, `global`, or `ephemeral`.
    pub scope: String,
    /// Whether this recipe mutates project files.
    #[serde(default)]
    pub mutates_project: bool,
    /// Whether this recipe requires network access.
    #[serde(default)]
    pub requires_network: bool,
}

fn reject_duplicate_ids<'a>(label: &str, ids: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(AuditError::ConfigValidation(format!(
                "{label} id must not be empty"
            )));
        }
        if !seen.insert(id) {
            return Err(AuditError::ConfigValidation(format!(
                "duplicate {label} id `{id}`"
            )));
        }
    }
    Ok(())
}

fn default_scope() -> String {
    "changed".into()
}

fn default_base() -> String {
    "HEAD".into()
}

fn default_timeout_seconds() -> u64 {
    120
}

fn default_max_findings() -> usize {
    20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_loads_checks_requirements_and_installers() {
        let config = AuditConfig::from_toml_str(
            r#"
            [defaults]
            scope = "workspace"
            max_findings = 10

            [profiles.ai_quality]
            description = "AI-generated code quality"
            checks = ["aislop"]

            [[checks]]
            id = "aislop"
            kind = "ai_quality"
            command = "aislop scan --changes --json"
            parser = "aislop_json"
            optional = true
            requires = ["aislop"]

            [[requirements]]
            id = "aislop"
            binary = "aislop"
            version_command = ["aislop", "version"]

            [[requirements.installers]]
            id = "npm-dev"
            label = "Add aislop as a project dev dependency"
            manager = "npm"
            command = ["npm", "install", "--save-dev", "aislop"]
            scope = "project"
            mutates_project = true
            requires_network = true
            "#,
        )
        .expect("config should parse");

        assert_eq!(config.defaults.scope, "workspace");
        assert_eq!(config.defaults.max_findings, 10);
        assert_eq!(config.checks.len(), 1);
        assert_eq!(
            config.profiles["ai_quality"].description.as_deref(),
            Some("AI-generated code quality")
        );
        assert_eq!(config.requirements[0].installers[0].id, "npm-dev");
    }

    #[test]
    fn config_rejects_duplicate_check_ids() {
        let err = AuditConfig::from_toml_str(
            r#"
            [[checks]]
            id = "same"
            kind = "lint"

            [[checks]]
            id = "same"
            kind = "test"
            "#,
        )
        .expect_err("duplicate check ids should fail");

        assert!(err.to_string().contains("duplicate check id `same`"));
    }

    #[test]
    fn config_rejects_unknown_profile_checks() {
        let err = AuditConfig::from_toml_str(
            r#"
            [profiles.quick]
            checks = ["missing"]
            "#,
        )
        .expect_err("unknown profile check should fail");

        assert!(err
            .to_string()
            .contains("profile `quick` references unknown check `missing`"));
    }

    #[test]
    fn config_rejects_unknown_requirement_references() {
        let err = AuditConfig::from_toml_str(
            r#"
            [[checks]]
            id = "semgrep"
            kind = "security"
            requires = ["semgrep"]
            "#,
        )
        .expect_err("unknown requirement should fail");

        assert!(err
            .to_string()
            .contains("references unknown requirement `semgrep`"));
    }
}
