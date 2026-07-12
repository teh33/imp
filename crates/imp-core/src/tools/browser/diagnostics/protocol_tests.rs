#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

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

#[test]
fn line_buffer_frames_split_and_final_chunks() {
    let mut lines = LineBuffer::default();
    lines.push("{\"id\":1}");
    assert_eq!(lines.pop(), None);
    lines.push("\n{\"id\":2}\r\n{\"id\":3}");
    assert_eq!(lines.pop().as_deref(), Some("{\"id\":1}"));
    assert_eq!(lines.pop().as_deref(), Some("{\"id\":2}"));
    assert_eq!(lines.pop(), None);
    lines.finish();
    assert_eq!(lines.pop().as_deref(), Some("{\"id\":3}"));
}

#[tokio::test]
async fn probe_uses_runtime_protocol_and_cleared_environment() {
    let script = r#"
[ "$1" = "mcp" ] || exit 11
[ "$2" = "--obey-robots" ] || exit 12
[ "$3" = "--block-private-networks" ] || exit 13
[ "$LIGHTPANDA_DISABLE_TELEMETRY" = "true" ] || exit 14
[ "$LIGHTPANDA_DISABLE_CORE_DUMP" = "1" ] || exit 15
IFS= read -r initialize || exit 17
IFS= read -r initialized || exit 18
IFS= read -r listed || exit 19
case "$initialize" in *'"method":"initialize"'*) ;; *) exit 20 ;; esac
case "$initialized" in *'"method":"notifications/initialized"'*) ;; *) exit 21 ;; esac
case "$listed" in *'"method":"tools/list"'*) ;; *) exit 22 ;; esac
printf '%s' '{"jsonrpc":"2.0","id":1,"result":{}}'
printf '\n%s' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"goto"},{"name":"extract"}]}}'
"#;
    let (_dir, binary) = fixture(script);
    let tools = probe_mcp_with_timeout(&binary, &BrowserConfig::default(), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(tools, ["goto", "extract"]);
}

#[tokio::test]
async fn probe_timeout_is_bounded() {
    let (_dir, binary) = fixture("exec /bin/sleep 60");
    let started = Instant::now();
    let error = probe_mcp_with_timeout(
        &binary,
        &BrowserConfig::default(),
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert_eq!(error, "Lightpanda MCP probe timed out");
    assert!(started.elapsed() < Duration::from_secs(2));
}
