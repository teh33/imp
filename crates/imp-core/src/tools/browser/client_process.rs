use std::collections::BTreeMap;
use std::path::Path;

use super::config::BrowserConfig;
use crate::process::{CommandSpec, ExecutionGrant, ProcessMode, ProcessRequest};

pub(super) const RETENTION_MARGIN_BYTES: usize = 64 * 1024;

pub(super) fn process_request(binary: &Path, config: &BrowserConfig, cwd: &Path) -> ProcessRequest {
    let environment = BTreeMap::from([
        ("LIGHTPANDA_DISABLE_TELEMETRY".into(), "true".into()),
        ("LIGHTPANDA_DISABLE_CORE_DUMP".into(), "1".into()),
    ]);
    let mut grant = ExecutionGrant::host(cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let mut arguments = vec!["mcp".into()];
    if config.obey_robots {
        arguments.push("--obey-robots".into());
    }
    if config.block_private_networks {
        arguments.push("--block-private-networks".into());
    }
    ProcessRequest {
        command: CommandSpec::new(binary.display().to_string()).with_arguments(arguments),
        cwd: cwd.to_path_buf(),
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: None,
        output_retention_bytes: config
            .max_response_bytes
            .saturating_add(RETENTION_MARGIN_BYTES),
        grant,
    }
}
