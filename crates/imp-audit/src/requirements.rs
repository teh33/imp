use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{AuditConfig, InstallRecipe};
use crate::error::{AuditError, Result};

/// Availability status for an external audit requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RequirementStatus {
    /// The required binary is available on PATH or at its configured path.
    Available,
    /// The required binary could not be found.
    Missing,
}

/// Probe used to determine whether a requirement binary is available.
pub trait RequirementProbe {
    /// Return true when `binary` can be executed or resolved.
    fn is_available(&self, binary: &str) -> bool;
}

/// PATH-based requirement probe for normal runtime discovery.
#[derive(Debug, Clone, Default)]
pub struct PathRequirementProbe;

impl RequirementProbe for PathRequirementProbe {
    fn is_available(&self, binary: &str) -> bool {
        command_exists(binary)
    }
}

/// Availability details for one requirement in a selected profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementAvailability {
    /// Requirement id from audit config.
    pub id: String,
    /// Binary expected on PATH.
    pub binary: String,
    /// Availability status.
    pub status: RequirementStatus,
    /// Checks that require this tool and are blocking if it is missing.
    #[serde(default)]
    pub required_by: Vec<String>,
    /// Checks that can be skipped if this tool is missing.
    #[serde(default)]
    pub optional_for: Vec<String>,
    /// User-approved installation options.
    #[serde(default)]
    pub installers: Vec<InstallRecipe>,
}

impl RequirementAvailability {
    /// Returns true when this missing requirement should block the selected profile.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.status == RequirementStatus::Missing && !self.required_by.is_empty()
    }
}

/// Build requirement availability rows for a profile.
pub fn availability_for_profile(
    config: &AuditConfig,
    profile: &str,
    probe: &impl RequirementProbe,
) -> Result<Vec<RequirementAvailability>> {
    let mut rows: BTreeMap<&str, RequirementAvailability> = BTreeMap::new();

    for check in config.checks_for_profile(profile)? {
        for requirement_id in &check.requires {
            let requirement = config
                .requirement(requirement_id)
                .ok_or_else(|| AuditError::UnknownItem(requirement_id.clone()))?;
            let row =
                rows.entry(requirement.id.as_str())
                    .or_insert_with(|| RequirementAvailability {
                        id: requirement.id.clone(),
                        binary: requirement.binary.clone(),
                        status: if probe.is_available(&requirement.binary) {
                            RequirementStatus::Available
                        } else {
                            RequirementStatus::Missing
                        },
                        required_by: Vec::new(),
                        optional_for: Vec::new(),
                        installers: requirement.installers.clone(),
                    });

            if check.optional {
                row.optional_for.push(check.id.clone());
            } else {
                row.required_by.push(check.id.clone());
            }
        }
    }

    Ok(rows.into_values().collect())
}

fn command_exists(binary: &str) -> bool {
    let path = Path::new(binary);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }

    let Some(paths) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&paths)
        .any(|dir| candidate_paths(&dir, binary).any(|p| is_executable_file(&p)))
}

fn candidate_paths(dir: &Path, binary: &str) -> impl Iterator<Item = PathBuf> {
    #[cfg(windows)]
    {
        let pathext = env::var_os("PATHEXT")
            .map(|value| {
                env::split_paths(&value)
                    .filter_map(|p| p.to_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![".exe".into(), ".cmd".into(), ".bat".into()]);
        let has_extension = Path::new(binary).extension().is_some();
        let mut candidates = vec![dir.join(binary)];
        if !has_extension {
            candidates.extend(
                pathext
                    .into_iter()
                    .map(|ext| dir.join(format!("{binary}{ext}"))),
            );
        }
        candidates.into_iter()
    }
    #[cfg(not(windows))]
    {
        std::iter::once(dir.join(binary))
    }
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    struct StaticProbe {
        available: HashSet<String>,
    }

    impl StaticProbe {
        fn with_available(binaries: &[&str]) -> Self {
            Self {
                available: binaries.iter().map(|binary| binary.to_string()).collect(),
            }
        }
    }

    impl RequirementProbe for StaticProbe {
        fn is_available(&self, binary: &str) -> bool {
            self.available.contains(binary)
        }
    }

    #[test]
    fn availability_marks_missing_optional_and_required_tools() {
        let config = AuditConfig::from_toml_str(
            r#"
            [profiles.quick]
            checks = ["optional-ai", "required-security"]

            [[checks]]
            id = "optional-ai"
            kind = "ai_quality"
            optional = true
            requires = ["aislop"]

            [[checks]]
            id = "required-security"
            kind = "security"
            requires = ["semgrep"]

            [[requirements]]
            id = "aislop"
            binary = "aislop"

            [[requirements.installers]]
            id = "npm-dev"
            label = "Install aislop"
            manager = "npm"
            command = ["npm", "install", "--save-dev", "aislop"]
            scope = "project"

            [[requirements]]
            id = "semgrep"
            binary = "semgrep"
            "#,
        )
        .expect("config should parse");

        let rows = availability_for_profile(&config, "quick", &StaticProbe::with_available(&[]))
            .expect("availability should resolve");

        let aislop = rows
            .iter()
            .find(|row| row.id == "aislop")
            .expect("aislop row");
        assert_eq!(aislop.status, RequirementStatus::Missing);
        assert!(!aislop.is_blocking());
        assert_eq!(aislop.optional_for, vec!["optional-ai"]);
        assert_eq!(aislop.installers[0].id, "npm-dev");

        let semgrep = rows
            .iter()
            .find(|row| row.id == "semgrep")
            .expect("semgrep row");
        assert_eq!(semgrep.status, RequirementStatus::Missing);
        assert!(semgrep.is_blocking());
        assert_eq!(semgrep.required_by, vec!["required-security"]);
    }

    #[test]
    fn availability_marks_present_tools() {
        let config = AuditConfig::from_toml_str(
            r#"
            [profiles.quick]
            checks = ["fmt"]

            [[checks]]
            id = "fmt"
            kind = "format"
            requires = ["cargo"]

            [[requirements]]
            id = "cargo"
            binary = "cargo"
            "#,
        )
        .expect("config should parse");

        let rows =
            availability_for_profile(&config, "quick", &StaticProbe::with_available(&["cargo"]))
                .expect("availability should resolve");

        assert_eq!(rows[0].status, RequirementStatus::Available);
        assert!(!rows[0].is_blocking());
    }
}
