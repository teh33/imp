use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{ProcessError, ProcessMode, ProcessRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkGrant {
    Denied,
    Allowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationRequirement {
    None,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessRestrictions {
    pub allow_children: bool,
}

impl Default for ProcessRestrictions {
    fn default() -> Self {
        Self { allow_children: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionGrant {
    pub readable_roots: BTreeSet<PathBuf>,
    pub writable_roots: BTreeSet<PathBuf>,
    pub network: NetworkGrant,
    pub allowed_environment: BTreeSet<String>,
    pub approved_secret_ids: BTreeSet<String>,
    pub approved_secret_environment: BTreeSet<String>,
    pub process_restrictions: ProcessRestrictions,
    pub isolation: IsolationRequirement,
}

impl ExecutionGrant {
    pub fn host(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        Self {
            readable_roots: BTreeSet::from([cwd.clone()]),
            writable_roots: BTreeSet::from([cwd]),
            network: NetworkGrant::Allowed,
            allowed_environment: std::env::vars_os()
                .filter_map(|(name, _)| name.into_string().ok())
                .collect(),
            approved_secret_ids: BTreeSet::new(),
            approved_secret_environment: BTreeSet::new(),
            process_restrictions: ProcessRestrictions::default(),
            isolation: IsolationRequirement::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementControl {
    WorkingDirectoryValidated,
    EnvironmentAllowlist,
    SecretEnvironmentValidated,
    ProcessGroup,
    FilesystemIsolation,
    NetworkIsolation,
    ChildProcessRestriction,
    Pty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnforcementSummary {
    pub backend: String,
    pub requested: BTreeSet<EnforcementControl>,
    pub enforced: BTreeSet<EnforcementControl>,
    pub preflight_validated: BTreeSet<EnforcementControl>,
}

pub trait ProcessBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn validate(&self, request: &ProcessRequest) -> Result<EnforcementSummary, ProcessError>;
}

#[derive(Debug, Default)]
pub struct HostProcessBackend;

impl ProcessBackend for HostProcessBackend {
    fn name(&self) -> &'static str {
        "host"
    }

    fn validate(&self, request: &ProcessRequest) -> Result<EnforcementSummary, ProcessError> {
        if request.mode == ProcessMode::Pty {
            return Err(ProcessError::UnsupportedCapability("pty"));
        }
        if request.grant.isolation == IsolationRequirement::Required {
            return Err(ProcessError::IsolationUnavailable);
        }
        validate_cwd(&request.cwd, &request.grant.readable_roots)?;
        validate_environment(request)?;

        let mut requested = BTreeSet::from([
            EnforcementControl::WorkingDirectoryValidated,
            EnforcementControl::EnvironmentAllowlist,
            EnforcementControl::SecretEnvironmentValidated,
            EnforcementControl::ProcessGroup,
        ]);
        if !request.grant.readable_roots.is_empty() || !request.grant.writable_roots.is_empty() {
            requested.insert(EnforcementControl::FilesystemIsolation);
        }
        if request.grant.network == NetworkGrant::Denied {
            requested.insert(EnforcementControl::NetworkIsolation);
        }
        if !request.grant.process_restrictions.allow_children {
            requested.insert(EnforcementControl::ChildProcessRestriction);
        }

        Ok(EnforcementSummary {
            backend: self.name().into(),
            requested,
            enforced: BTreeSet::from([
                EnforcementControl::EnvironmentAllowlist,
                EnforcementControl::SecretEnvironmentValidated,
                EnforcementControl::ProcessGroup,
            ]),
            preflight_validated: BTreeSet::from([EnforcementControl::WorkingDirectoryValidated]),
        })
    }
}

fn validate_cwd(cwd: &Path, readable_roots: &BTreeSet<PathBuf>) -> Result<(), ProcessError> {
    let canonical = std::fs::canonicalize(cwd).map_err(ProcessError::Io)?;
    let allowed = readable_roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .is_ok_and(|canonical_root| canonical.starts_with(canonical_root))
    });
    if allowed {
        Ok(())
    } else {
        Err(ProcessError::GrantDenied("working directory is outside readable roots".into()))
    }
}

fn validate_environment(request: &ProcessRequest) -> Result<(), ProcessError> {
    for name in request.environment.keys() {
        if !request.grant.allowed_environment.contains(name) {
            return Err(ProcessError::GrantDenied(format!(
                "environment variable `{name}` is not allowed"
            )));
        }
    }
    for secret in &request.approved_secret_environment {
        if !request.grant.approved_secret_ids.contains(&secret.secret_id)
            || !request.grant.approved_secret_environment.contains(&secret.name)
        {
            return Err(ProcessError::GrantDenied(format!(
                "secret environment `{}` is not approved",
                secret.name
            )));
        }
    }
    Ok(())
}
