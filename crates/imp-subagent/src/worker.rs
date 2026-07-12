use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use imp_process::{CommandSpec, ManagedChild, StreamMode};
use serde_json::Value;

use crate::events::{fail, handle_event, mark_worker_failed, remove_socket, send_child};
use crate::executor::{Request, Response};
use crate::model::{Error, Result, Status};
use crate::store::{load_record, now_ms, save_record, timed_out};

const MAX_EVENT_BYTES: usize = 256 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run_worker(state: &Path) -> Result<()> {
    let result = run_worker_inner(state);
    if let Err(error) = &result {
        let _ = mark_worker_failed(state, error);
        let _ = remove_socket(state);
    }
    result
}

fn run_worker_inner(state: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        worker_unix(state)
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        Err(Error::Unavailable(
            "imp-subagent workers require Unix sockets on this platform".into(),
        ))
    }
}

#[cfg(unix)]
fn worker_unix(state: &Path) -> Result<()> {
    use std::os::unix::net::UnixListener;
    let mut record = load_record(state)?;
    if record.socket.exists() {
        fs::remove_file(&record.socket)?;
    }
    let listener = UnixListener::bind(&record.socket)?;
    listener.set_nonblocking(true)?;
    let mut command = CommandSpec::new(&record.executable);
    command.args = record.child_args.clone();
    command.args.extend([
        "--mode".into(),
        "rpc".into(),
        "--session".into(),
        record.artifacts.session.to_string_lossy().into_owned(),
    ]);
    if let Some(model) = &record.model {
        command.args.extend(["--model".into(), model.clone()]);
    }
    command.environment = record.environment.clone();
    command.cwd = Some(record.cwd.clone());
    command.stdin = StreamMode::Piped;
    command.stdout = StreamMode::Piped;
    command.stderr_file = Some(record.artifacts.stderr.clone());
    command.process_group = true;
    let mut child = ManagedChild::spawn(&command)
        .map_err(|error| Error::Process(format!("failed to start child imp: {error}")))?;
    let mut stdin = child.take_stdin().map_err(process_error)?;
    let stdout = child.take_stdout().map_err(process_error)?;
    let (events_tx, events_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
            if line.len() <= MAX_EVENT_BYTES {
                if let Ok(event) = serde_json::from_str(&line) {
                    let _ = events_tx.send(event);
                }
            }
        }
    });
    wait_rpc_ready(&mut child, &events_rx)?;
    record.status = Status::Running;
    record.updated_at_ms = now_ms();
    save_record(&record)?;
    let mut active_turn = None;
    let mut result = String::new();
    loop {
        while let Ok(event) = events_rx.try_recv() {
            handle_event(&mut record, &mut active_turn, &mut result, event)?;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let request: Request = serde_json::from_reader(&stream)?;
                let mut stop = false;
                let response = match request {
                    Request::Ping => Response {
                        ok: true,
                        turn: active_turn.clone(),
                        error: None,
                    },
                    Request::Prompt { prompt } if active_turn.is_none() => {
                        let turn = send_child(&mut stdin, "prompt", prompt)?;
                        active_turn = Some(turn.clone());
                        record.turn_id = Some(turn.clone());
                        Response {
                            ok: true,
                            turn: Some(turn),
                            error: None,
                        }
                    }
                    Request::FollowUp { prompt } => {
                        let turn = active_turn
                            .clone()
                            .unwrap_or_else(|| format!("turn_{}", now_ms()));
                        send_child(&mut stdin, "followup", prompt)?;
                        active_turn = Some(turn.clone());
                        record.turn_id = Some(turn.clone());
                        record.status = Status::Running;
                        record.updated_at_ms = now_ms();
                        save_record(&record)?;
                        Response {
                            ok: true,
                            turn: Some(turn),
                            error: None,
                        }
                    }
                    Request::Prompt { .. } => Response {
                        ok: false,
                        turn: active_turn.clone(),
                        error: Some("child already has an active turn".into()),
                    },
                    Request::Stop => {
                        stop = true;
                        Response {
                            ok: true,
                            turn: active_turn.clone(),
                            error: None,
                        }
                    }
                };
                serde_json::to_writer(&mut stream, &response)?;
                stream.write_all(b"\n")?;
                if stop {
                    record.status = Status::Cancelled;
                    record.summary = Some("cancelled by parent".into());
                    record.updated_at_ms = now_ms();
                    save_record(&record)?;
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if timed_out(&record) {
            record.status = Status::Cancelled;
            record.summary = Some("subagent resource timeout reached".into());
            record
                .diagnostics
                .push("timeout_seconds limit reached".into());
            record.updated_at_ms = now_ms();
            save_record(&record)?;
            break;
        }
        if let Some(status) = child.try_wait().map_err(process_error)? {
            fail(
                &mut record,
                format!("child exited unexpectedly with {status}"),
            )?;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.terminate(Duration::from_secs(2));
    let _ = fs::remove_file(&record.socket);
    Ok(())
}

#[cfg(unix)]
fn wait_rpc_ready(child: &mut ManagedChild, events: &Receiver<Value>) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) if rpc_ready_compatible(&event) => return Ok(()),
            Ok(event) if event["type"] == "rpc_ready" => {
                return Err(Error::Protocol(format!(
                    "incompatible child RPC readiness: {event}"
                )))
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Process(
                    "child closed RPC output during startup".into(),
                ))
            }
        }
        if let Some(status) = child.try_wait().map_err(process_error)? {
            return Err(Error::Process(format!(
                "child exited during RPC startup with {status}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(Error::Process(
                "timed out waiting for child RPC readiness".into(),
            ));
        }
    }
}

fn rpc_ready_compatible(event: &Value) -> bool {
    const REQUIRED: &[&str] = &["prompt", "followup", "cancel"];
    let Some(capabilities) = event["capabilities"].as_array() else {
        return false;
    };
    event["type"] == "rpc_ready"
        && event["protocol"] == "imp-rpc"
        && event["version"] == 1
        && REQUIRED.iter().all(|required| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some(required))
        })
}

fn process_error(error: imp_process::Error) -> Error {
    Error::Process(error.to_string())
}
#[cfg(test)]
mod tests {
    use super::rpc_ready_compatible;

    #[test]
    fn readiness_requires_protocol_version_and_used_capabilities() {
        let valid = serde_json::json!({
            "type": "rpc_ready",
            "protocol": "imp-rpc",
            "version": 1,
            "capabilities": ["prompt", "followup", "cancel", "future"]
        });
        assert!(rpc_ready_compatible(&valid));
        for invalid in [
            serde_json::json!({"type":"rpc_ready","protocol":"imp-rpc","version":2,"capabilities":["prompt","followup","cancel"]}),
            serde_json::json!({"type":"rpc_ready","protocol":"imp-rpc","version":1,"capabilities":["prompt","cancel"]}),
        ] {
            assert!(!rpc_ready_compatible(&invalid));
        }
    }
}
