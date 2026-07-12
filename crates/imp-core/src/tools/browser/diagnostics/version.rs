use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::process::{
    CommandSpec, ExecutionGrant, OutputCursor, OutputStream, ProcessManager, ProcessMode,
    ProcessRequest,
};

const OUTPUT_RETENTION_BYTES: usize = 64 * 1024;

pub(super) async fn read_version(binary: &Path, timeout: Duration) -> Result<String, String> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let mut grant = ExecutionGrant::host(&cwd);
    grant.allowed_environment = environment.keys().cloned().collect();
    let request = ProcessRequest {
        command: CommandSpec::new(binary.display().to_string()).with_arguments(["version".into()]),
        cwd,
        environment,
        approved_secret_environment: Vec::new(),
        mode: ProcessMode::Pipes,
        timeout: Some(timeout),
        output_retention_bytes: OUTPUT_RETENTION_BYTES,
        grant,
    };
    let manager = ProcessManager::new();
    let process = manager
        .start(request)
        .await
        .map_err(|error| format!("could not run Lightpanda version: {error}"))?;
    manager
        .close_stdin(process.id)
        .await
        .map_err(|error| format!("could not run Lightpanda version: {error}"))?;
    let mut cursor = OutputCursor::start(process.id);
    let mut version = String::new();
    loop {
        let output = manager
            .observe(
                process.id,
                cursor,
                timeout + Duration::from_secs(1),
                usize::MAX,
            )
            .await
            .map_err(|error| format!("could not read Lightpanda version: {error}"))?;
        cursor = output.next_cursor;
        for chunk in output.chunks {
            if chunk.stream == OutputStream::Stdout {
                version.push_str(&chunk.text);
            }
        }
        if output.state.is_terminal() && !output.response_truncated {
            break;
        }
    }
    let exit = manager
        .wait(process.id)
        .await
        .map_err(|error| format!("could not wait for Lightpanda version: {error}"))?;
    if exit.timed_out {
        return Err("Lightpanda version check timed out".into());
    }
    if exit.code != Some(0) {
        return Err(format!(
            "Lightpanda version failed with status {}",
            exit.code.map_or_else(
                || "unknown exit status".into(),
                |code| format!("exit status: {code}"),
            )
        ));
    }
    let version = version.trim().to_string();
    if version.is_empty() {
        return Err("Lightpanda version returned no version".into());
    }
    Ok(version)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    fn fixture(script: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("lightpanda");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).unwrap();
        (dir, binary)
    }

    #[tokio::test]
    async fn reads_version_through_process_runtime() {
        let (_dir, binary) = fixture("printf '0.3.4\\n'");
        let version = read_version(&binary, Duration::from_secs(1)).await.unwrap();
        assert_eq!(version, "0.3.4");
    }

    #[tokio::test]
    async fn reports_nonzero_version_exit() {
        let (_dir, binary) = fixture("exit 7");
        let error = read_version(&binary, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "Lightpanda version failed with status exit status: 7"
        );
    }

    #[tokio::test]
    async fn bounds_version_probe_timeout() {
        let (_dir, binary) = fixture("exec sleep 60");
        let started = Instant::now();
        let error = read_version(&binary, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(error, "Lightpanda version check timed out");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
